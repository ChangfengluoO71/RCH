import 'dart:async';
import 'dart:ui' as ui;

import 'package:app/src/rust/api/source.dart';
import 'package:app/store/qr_image_saver.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:qr_flutter/qr_flutter.dart';

/// 夸克网页扫码登录（与 115 的 `scanCloud115Cookie` 同形）：
/// 手机**夸克 App** 扫码确认后返回 Cookie 字符串，调用方负责保存到书源。
///
/// 为什么需要：夸克书源过去只能让用户开 F12 从网页端复制 Cookie（易错、易过期后无感），
/// 而夸克 passport 有公开的二维码登录接口（见 LOG 第 66 轮的接口依据）。
Future<String?> scanQuarkCookie(
  BuildContext context, {
  void Function(String message)? onError,
}) async {
  try {
    final payload = await quarkQrStart().timeout(const Duration(seconds: 20));
    if (!context.mounted) return null;
    return await showDialog<String>(
      context: context,
      barrierDismissible: false,
      builder: (c) => QuarkQrScanDialog(payload: payload),
    );
  } catch (error) {
    onError?.call('获取夸克二维码失败：$error');
    return null;
  }
}

/// 夸克扫码对话框：展示二维码并轮询扫码状态，确认后自动换取 Cookie 并关闭。
class QuarkQrScanDialog extends StatefulWidget {
  const QuarkQrScanDialog({super.key, required this.payload});

  final QuarkQrPayload payload;

  @override
  State<QuarkQrScanDialog> createState() => _QuarkQrScanDialogState();
}

class _QuarkQrScanDialogState extends State<QuarkQrScanDialog> {
  /// 截屏用的边界（保存相册时取真实 PNG 字节）。
  final GlobalKey _qrKey = GlobalKey();

  Timer? _timer;
  String _status = '请用手机夸克 App 扫码并确认';
  bool _finished = false;

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(const Duration(seconds: 2), (_) => _poll());
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _poll() async {
    if (_finished || !mounted) return;
    try {
      final status = await quarkQrPoll(
        token: widget.payload.token,
        requestId: widget.payload.requestId,
      );
      if (!mounted || _finished) return;
      if (status == 2) {
        _finished = true;
        _timer?.cancel();
        setState(() => _status = '已确认，正在获取登录状态…');
        final cookie = await quarkQrResult(
          token: widget.payload.token,
          requestId: widget.payload.requestId,
        );
        if (mounted) Navigator.of(context).pop(cookie);
        return;
      }
      if (status == -1) {
        _finished = true;
        _timer?.cancel();
        setState(() => _status = '二维码已失效或被取消，请关闭后重试');
        return;
      }
      setState(() => _status = '等待扫码…（请用手机夸克 App 扫一扫）');
    } catch (error) {
      if (!mounted) return;
      setState(() => _status = '轮询失败：$error');
    }
  }


  /// 第 73 轮：把当前二维码保存到手机相册（`gal` 走系统媒体库 ✓）。
  ///
  /// 为什么之前"没有"：`store/qr_image_saver.dart` 早就实现了保存，
  /// 但**从来没有任何调用点**（既有提交 1bf2e37 引入后一直是孤儿），这里是接线。
  Future<void> _saveQrToGallery(BuildContext c) async {
    final messenger = ScaffoldMessenger.of(c);
    try {
      // 第 76 轮修正：`QrPainter.toImageData` 返回的是**原始 RGBA 像素**，
      // 不是 PNG 文件 ⇒ 之前存进相册的图无法识别（看着全黑）。
      // 改为截取屏幕上的二维码边界并导出 **PNG** 字节。
      final boundary = _qrKey.currentContext?.findRenderObject()
          as RenderRepaintBoundary?;
      final image = await boundary?.toImage(pixelRatio: 3);
      final data = await image?.toByteData(format: ui.ImageByteFormat.png);
      final bytes = data?.buffer.asUint8List();
      if (bytes == null || bytes.isEmpty) {
        throw StateError('二维码渲染失败');
      }
      await saveQrImageToGallery(bytes);
      messenger.showSnackBar(const SnackBar(content: Text('二维码已保存到相册（相册 RCH）')));
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('保存失败：$e')));
    }
  }
  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('夸克扫码登录'),
      content: SizedBox(
        width: 300,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            RepaintBoundary(
              key: _qrKey,
              child: Container(
                color: Theme.of(context).colorScheme.onSurface,
                padding: const EdgeInsets.all(8),
                child: QrImageView(
                  data: widget.payload.qrcode,
                  size: 220,
                  backgroundColor: Theme.of(context).colorScheme.onSurface,
                ),
            ),
            ),
            const SizedBox(height: 12),
            Text(_status, textAlign: TextAlign.center),
            const SizedBox(height: 6),
            Text(
              '用手机夸克 App 扫一扫并确认；确认后本机直接拿到登录 Cookie，'
              '不需要 F12 复制。',
              style: TextStyle(fontSize: 11, color: Theme.of(context).colorScheme.onSurfaceVariant),
              textAlign: TextAlign.center,
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => _saveQrToGallery(context),
          child: const Text('保存到相册'),
        ),
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
      ],
    );
  }
}
