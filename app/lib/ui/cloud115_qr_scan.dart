import 'dart:async';
import 'dart:ui' as ui;

import 'package:app/src/rust/api/source.dart';
import 'package:app/store/qr_image_saver.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:qr_flutter/qr_flutter.dart';

/// 发起 115 网页扫码并返回 Cookie（`UID=...; CID=...`，末尾不带 `;`）。
/// 获取二维码失败 / 用户取消 / 扫码失败返回 null；中途错误经 [onError] 回调。
///
/// 供「添加书源」「编辑书源」「Cookie 失效自动续期」三处复用，
/// 保证扫码对话框只有一份实现。
Future<String?> scanCloud115Cookie(
  BuildContext context, {
  String app = 'wechatmini',
  void Function(String message)? onError,
}) async {
  try {
    final qr = await cloud115CookieQrStart().timeout(const Duration(seconds: 20));
    if (!context.mounted) return null;
    final cookie = await showDialog<String>(
      context: context,
      builder: (c) => Cloud115CookieQrScanDialog(
        uid: qr.uid,
        time: qr.time,
        sign: qr.sign,
        qrcode: qr.qrcode,
        app: app,
        onError: onError,
      ),
    );
    if (cookie == null || cookie.trim().isEmpty) return null;
    return cookie;
  } on TimeoutException {
    onError?.call('获取 115 二维码超时（请检查网络/代理后重试）');
  } catch (e) {
    onError?.call('获取 115 二维码失败（请检查网络/代理后重试）:$e');
  }
  return null;
}

/// 115 网页扫码获取 Cookie 对话框：渲染二维码、轮询状态，成功后自动换取 Cookie。
/// 扫码成功以 `Navigator.pop(cookie)` 返回。
///
/// 注意：不能用 QrImageView——其内部 LayoutBuilder 与 AlertDialog 的
/// IntrinsicWidth 冲突，performLayout 抛异常导致对话框只剩遮罩。
/// 统一用 QrPainter + CustomPaint（有回归测试护栏，勿回退）。
class Cloud115CookieQrScanDialog extends StatefulWidget {
  final String uid;
  final PlatformInt64 time;
  final String sign;
  final String qrcode;
  final String app;
  final void Function(String msg)? onError;

  const Cloud115CookieQrScanDialog({
    super.key,
    required this.uid,
    required this.time,
    required this.sign,
    required this.qrcode,
    required this.app,
    this.onError,
  });

  @override
  State<Cloud115CookieQrScanDialog> createState() =>
      _Cloud115CookieQrScanDialogState();
}

class _Cloud115CookieQrScanDialogState extends State<Cloud115CookieQrScanDialog> {
  /// 截屏用的边界（保存相册时取真实 PNG 字节）。
  final GlobalKey _qrKey = GlobalKey();

  final ValueNotifier<String> _status = ValueNotifier('请用 115 APP 扫码');
  Timer? _timer;
  bool _polling = false;

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(const Duration(seconds: 2), (_) => _pollOnce());
    _pollOnce();
  }

  @override
  void dispose() {
    _timer?.cancel();
    _status.dispose();
    super.dispose();
  }

  Future<void> _pollOnce() async {
    if (_polling) return; // 防止上次请求未返回时并发轮询
    _polling = true;
    try {
      final status = await cloud115CookieQrPoll(
          uid: widget.uid, time: widget.time, sign: widget.sign);
      if (!mounted) return;
      if (status == 2) {
        _timer?.cancel();
        try {
          final cookie =
              await cloud115CookieQrResult(uid: widget.uid, app: widget.app);
          if (!mounted) return;
          Navigator.of(context).pop(cookie);
        } catch (e) {
          if (!mounted) return;
          widget.onError?.call('获取 Cookie 失败（请检查网络后重新扫码）:$e');
          Navigator.of(context).pop();
        }
        return;
      }
      if (status == 1) {
        _status.value = '已扫码，请在手机上确认';
      } else if (status == -1) {
        _timer?.cancel();
        widget.onError?.call('二维码已过期，请重新获取');
        Navigator.of(context).pop();
      } else if (status == -2) {
        _timer?.cancel();
        widget.onError?.call('已取消扫码');
        Navigator.of(context).pop();
      }
    } catch (e) {
      if (!mounted) return;
      _status.value = '查询状态失败:$e，继续等待…';
    } finally {
      _polling = false;
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
  Widget build(BuildContext c) => AlertDialog(
        scrollable: true,
        title: const Text('115 扫码获取 Cookie'),
        content: Column(mainAxisSize: MainAxisSize.min, children: [
          Text('用 115 手机 App 扫码，无需申请 APP ID',
              style: TextStyle(fontSize: 12, color: Theme.of(context).colorScheme.onSurfaceVariant)),
          const SizedBox(height: 10),
          RepaintBoundary(
            key: _qrKey,
            child: Container(
              color: const Color(0xFFFFFFFF),
              padding: const EdgeInsets.all(8),
              child: SizedBox(
                width: 220,
                height: 220,
                child: CustomPaint(
                  painter: QrPainter(
                    data: widget.qrcode,
                    version: QrVersions.auto,
                  ),
                ),
              ),
            ),
          ),
          const SizedBox(height: 10),
          // 第 76 轮（用户反馈）：115 建议用**微信小程序**扫码，
          // 用 115 App 扫可能提示"不支持的设备/二维码已过期"。
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
            decoration: BoxDecoration(
              color: const Color(0x332E7D32),
              borderRadius: BorderRadius.circular(6),
            ),
            child: const Text(
              '推荐用「微信」扫码 → 小程序「115 网盘」→ 确认登录（用 115 App 扫可能提示设备不支持）',
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12, height: 1.5),
            ),
          ),
          const SizedBox(height: 10),
          ValueListenableBuilder<String>(
            valueListenable: _status,
            builder: (_, s, _) => Text(s, style: const TextStyle(fontSize: 12)),
          ),
        ]),
        actions: [
          TextButton(
            onPressed: () => _saveQrToGallery(c),
            child: const Text('保存到相册'),
          ),
          TextButton(onPressed: () => Navigator.of(c).pop(), child: const Text('关闭')),
        ],
      );
}
