import 'dart:typed_data';

import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/baidu_session.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/quark_session.dart';
import 'package:app/store/sftp_session.dart';
import 'package:app/store/webdav_session.dart';
import 'package:flutter/material.dart';

/// 封面编辑器:翻页选一帧,拖框裁剪,保存为该漫画封面(页码 + 相对裁剪区域)。
class CoverEditorPage extends StatefulWidget {
  final BookSource source;
  final String path;
  final String title;

  const CoverEditorPage({
    super.key,
    required this.source,
    required this.path,
    required this.title,
  });

  @override
  State<CoverEditorPage> createState() => _CoverEditorPageState();
}

class _CoverEditorPageState extends State<CoverEditorPage> {
  BookInfo? _book;
  int _page = 0;
  Uint8List? _bytes;
  String? _error;

  Rect? _crop; // 裁剪框(相对图片 0-1),null=整页
  Size _imgSize = Size.zero; // 图片原始像素尺寸
  Offset? _dragStart;

  @override
  void initState() {
    super.initState();
    _open();
  }

  Future<void> _open() async {
    try {
      final s = widget.source;
      final strategy = LibraryStore.instance.settings.bookOpenStrategy.name;
      // 封面编辑是纯本地操作：先试 raw/ 缓存，命中直接本地打开、不联网；
      // 未命中再按全局打开策略（auto/download/stream）走远程。
      final b = (await _openCached()) ??
          switch (s.type) {
            'webdav' => await openWebdavBook(
                session: await webdavSessionFor(s),
                path: widget.path,
                strategy: strategy),
            'sftp' => await openSftpBook(
                session: await sftpSessionFor(s),
                path: widget.path,
                strategy: strategy),
            'baidu' => await openBaiduBook(
                session: await baiduSessionFor(s),
                path: widget.path,
                strategy: strategy),
            '115' => await openCloud115BookFor(s,
                session: await cloud115SessionFor(s),
                path: widget.path,
                strategy: strategy),
            'quark' => await openQuarkBook(
                session: await quarkSessionFor(s),
                path: widget.path,
                strategy: strategy),
            _ => await openLocalBook(path: widget.path),
          };
      if (!mounted) return;
      setState(() => _book = b);
      await _load(0);
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    }
  }

  /// 尝试从 raw/ 本地缓存直接打开远程书；无缓存或打开失败返回 null（由 _open 回退远程）。
  Future<BookInfo?> _openCached() async {
    final s = widget.source;
    if (!s.needsSession) return null;
    try {
      final session = switch (s.type) {
        'webdav' => await webdavSessionFor(s),
        'sftp' => await sftpSessionFor(s),
        'baidu' => await baiduSessionFor(s),
        '115' => await cloud115SessionFor(s),
        'quark' => await quarkSessionFor(s),
        _ => null,
      };
      if (session == null) return null;
      return await openCachedRemoteBook(
          kind: s.type, session: session, path: widget.path);
    } catch (_) {
      return null; // 缓存路径失败（如会话未建立）时交给远程策略兜底
    }
  }

  Future<void> _load(int page) async {
    final b = _book;
    if (b == null || page < 0 || page >= b.pageCount) return;
    final bytes = await bookPage(handle: b.handle, index: page);
    if (!mounted) return;
    _resolveSize(bytes);
    setState(() {
      _page = page;
      _bytes = bytes;
      _crop = null; // 翻页重置裁剪框
    });
  }

  /// 解码图片拿到原始尺寸(用于坐标换算)。
  void _resolveSize(Uint8List bytes) {
    final stream = MemoryImage(bytes).resolve(const ImageConfiguration());
    late ImageStreamListener listener;
    listener = ImageStreamListener((info, _) {
      if (mounted) {
        setState(() => _imgSize = Size(
            info.image.width.toDouble(), info.image.height.toDouble()));
      }
      stream.removeListener(listener);
    });
    stream.addListener(listener);
  }

  void _save() {
    final meta = LibraryStore.instance.metaOf(widget.source, widget.path);
    meta.coverPage = _page;
    if (_crop != null) {
      meta.cropX = _crop!.left;
      meta.cropY = _crop!.top;
      meta.cropW = _crop!.width;
      meta.cropH = _crop!.height;
    }
    LibraryStore.instance.updateMeta(meta);
    Navigator.of(context).pop();
  }

  void _clearCrop() {
    final meta = LibraryStore.instance.metaOf(widget.source, widget.path);
    meta.coverPage = _page;
    meta.cropX = meta.cropY = meta.cropW = meta.cropH = null;
    LibraryStore.instance.updateMeta(meta);
    setState(() => _crop = null);
  }

  @override
  Widget build(BuildContext context) {
    final b = _book;
    return Scaffold(
      appBar: AppBar(
        title: Text('自定义封面 ${b == null ? '' : '(${_page + 1}/${b.pageCount})'}'),
        actions: [
          TextButton(onPressed: _clearCrop, child: const Text('清除裁剪')),
          const SizedBox(width: 8),
        ],
      ),
      body: Column(
        children: [
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 6),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                IconButton(
                    icon: const Icon(Icons.chevron_left), onPressed: () => _load(_page - 1)),
                Text(b == null ? '…' : '${_page + 1} / ${b.pageCount}'),
                IconButton(
                    icon: const Icon(Icons.chevron_right), onPressed: () => _load(_page + 1)),
              ],
            ),
          ),
          Expanded(
            child: _error != null
                ? Center(child: Text('错误: $_error'))
                : _bytes == null
                    ? const Center(child: CircularProgressIndicator())
                    : LayoutBuilder(
                        builder: (context, cons) {
                          // BoxFit.contain:计算图片在容器中的实际显示区域。
                          double dw = 0, dh = 0;
                          if (_imgSize.width > 0 && _imgSize.height > 0) {
                            final scale = (cons.maxWidth / _imgSize.width)
                                .clamp(0.0, cons.maxHeight / _imgSize.height);
                            dw = _imgSize.width * scale;
                            dh = _imgSize.height * scale;
                          }
                          final dx = (cons.maxWidth - dw) / 2;
                          final dy = (cons.maxHeight - dh) / 2;
                          return Stack(
                            children: [
                              Positioned(
                                left: dx,
                                top: dy,
                                width: dw,
                                height: dh,
                                child: GestureDetector(
                                  onPanStart: (d) =>
                                      setState(() => _dragStart = d.localPosition),
                                  onPanUpdate: (d) {
                                    if (_dragStart == null || dw == 0 || dh == 0) return;
                                    final r = Rect.fromPoints(_dragStart!, d.localPosition);
                                    setState(() {
                                      _crop = Rect.fromLTRB(
                                        (r.left / dw).clamp(0.0, 1.0),
                                        (r.top / dh).clamp(0.0, 1.0),
                                        (r.right / dw).clamp(0.0, 1.0),
                                        (r.bottom / dh).clamp(0.0, 1.0),
                                      );
                                    });
                                  },
                                  onPanEnd: (_) => _dragStart = null,
                                  child: Image.memory(_bytes!, fit: BoxFit.fill),
                                ),
                              ),
                              if (_crop != null)
                                Positioned(
                                  left: dx + _crop!.left * dw,
                                  top: dy + _crop!.top * dh,
                                  width: _crop!.width * dw,
                                  height: _crop!.height * dh,
                                  child: Container(
                                    decoration: BoxDecoration(
                                      border: Border.all(color: Colors.amber, width: 2),
                                      color: Colors.amber.withValues(alpha: 0.15),
                                    ),
                                  ),
                                ),
                            ],
                          );
                        },
                      ),
          ),
          SafeArea(
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: SizedBox(
                width: double.infinity,
                child: FilledButton.icon(
                  onPressed: _save,
                  icon: const Icon(Icons.check),
                  label: Text(_crop == null ? '用整页做封面' : '保存裁剪为封面'),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}
