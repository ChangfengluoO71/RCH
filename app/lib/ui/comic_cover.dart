import 'dart:async';
import 'dart:ui' as ui;

import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/baidu_session.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/quark_session.dart';
import 'package:app/store/sftp_session.dart';
import 'package:app/ui/common.dart';
import 'package:app/store/webdav_session.dart';
import 'package:flutter/material.dart';

/// 封面加载任务队列 — 限制并发 FFI 调用数，避免数百个封面同时竞争线程池。
///
/// 设计：
/// - 最大并发数 4：本地封面 open_document + decode 约 30-80ms/本，4 并发足以喂饱 GPU。
/// - 已缓存的任务立即返回（内存缓存命中），不消耗并发槽位。
/// - 滚动时新出现的 Widget 入队，不再可见的 Widget 自动取消（didUpdateWidget dispose）。
class _CoverLoadQueue {
  _CoverLoadQueue._();

  static final _CoverLoadQueue instance = _CoverLoadQueue._();

  static const int maxConcurrent = 4;

  int _running = 0;
  final List<_QueuedTask> _pending = [];

  Completer<ui.Image> enqueue(String key, Future<ui.Image> Function() task) {
    final c = Completer<ui.Image>();
    final qt = _QueuedTask(key: key, task: task, completer: c);
    _pending.add(qt);
    _drain();
    return c;
  }

  void cancel(String key) {
    _pending.removeWhere((qt) {
      if (qt.key == key) {
        if (!qt.completer.isCompleted) {
          qt.completer.completeError(Exception('cancelled'));
        }
        return true;
      }
      return false;
    });
  }

  void _drain() {
    while (_running < maxConcurrent && _pending.isNotEmpty) {
      final qt = _pending.removeAt(0);
      if (qt.completer.isCompleted) continue; // 已被 cancel
      _running++;
      qt.task().then((img) {
        qt.completer.complete(img);
      }).catchError((e) {
        if (!qt.completer.isCompleted) qt.completer.completeError(e);
      }).whenComplete(() {
        _running--;
        _drain();
      });
    }
  }
}

class _QueuedTask {
  final String key;
  final Future<ui.Image> Function() task;
  final Completer<ui.Image> completer;
  _QueuedTask({required this.key, required this.task, required this.completer});
}

/// 漫画封面：统一本地 / WebDAV 来源，带全局内存缓存。
///
/// StatefulWidget 设计确保：
/// - 加载 Future 只在 initState 中创建一次，父 rebuild 不会重新触发加载。
/// - 并发限制 4 个 FFI 调用 + 队列调度。
/// - 内存缓存命中立即返回，不经过队列。
/// - 滚动时 Widget dispose 自动取消队列中的等待任务。
/// - WebDAV 封面懒加载：未打开过的漫画不主动请求封面。
class ComicCover extends StatefulWidget {
  final BookSource source;
  final String path;
  final BoxFit fit;
  final bool force;

  const ComicCover({
    super.key,
    required this.source,
    required this.path,
    this.fit = BoxFit.cover,
    this.force = false,
  });

  @override
  State<ComicCover> createState() => _ComicCoverState();

  // ---- 全局内存缓存（已完成的封面） ----

  static final Map<String, ui.Image> _cache = {};

  static void clear() => _cache.clear();

  static void evict(String key) => _cache.remove(key);

  static void evictAll(String sourceId, String path) {
    _cache.removeWhere((k, _) => k.startsWith('$sourceId|$path'));
  }
}

class _ComicCoverState extends State<ComicCover> {
  Future<ui.Image>? _future;

  String get _cacheKey {
    final store = LibraryStore.instance;
    final q = store.settings.coverQuality;
    final meta = store.metaOf(widget.source, widget.path);
    return '${widget.source.id}|${widget.path}|${q.name}|${meta.coverPage}'
        '|${meta.cropX},${meta.cropY},${meta.cropW},${meta.cropH}';
  }

  bool get _shouldSkipLoad {
    if (!widget.source.needsSession) return false;
    if (widget.force) return false;
    final key = bookKeyOf(widget.source.type, widget.source.id, widget.path);
    return !LibraryStore.instance.records.containsKey(key);
  }

  @override
  void initState() {
    super.initState();
    _maybeLoad();
  }

  @override
  void didUpdateWidget(covariant ComicCover oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.source.id != widget.source.id ||
        oldWidget.path != widget.path ||
        oldWidget.force != widget.force) {
      // 路径变化：取消旧队列任务，重新加载
      _CoverLoadQueue.instance.cancel(_cacheKey);
      _future = null;
      _maybeLoad();
    }
  }

  @override
  void dispose() {
    // Widget 不可见时取消队列中的等待任务（已经开始的 FFI 调用不中断）
    _CoverLoadQueue.instance.cancel(_cacheKey);
    super.dispose();
  }

  void _maybeLoad() {
    if (_future != null) return;
    if (_shouldSkipLoad) return;

    final key = _cacheKey;

    // 内存缓存命中 → 立即完成
    final cached = ComicCover._cache[key];
    if (cached != null) {
      _future = Future.value(cached);
      return;
    }

    // 入队：并发控制在队列内部
    final completer = _CoverLoadQueue.instance.enqueue(key, _load);
    _future = completer.future;
    _future!.then((img) {
      ComicCover._cache[key] = img;
    }).catchError((_) {});
  }

  /// 实际的封面加载逻辑（不包含队列调度）。
  Future<ui.Image> _load() async {
    final store = LibraryStore.instance;
    final q = store.settings.coverQuality;
    final (w, h) = q.size;
    final meta = store.metaOf(widget.source, widget.path);
    final crop = meta.hasCrop
        ? CropRect(x: meta.cropX!, y: meta.cropY!, w: meta.cropW!, h: meta.cropH!)
        : null;

    if (widget.source.isWebDav) {
      try {
        final session = await webdavSessionFor(widget.source);
        final hasRaw = await webdavHasRawCache(
            session: session, path: widget.path);
        if (!hasRaw) throw Exception('no raw cache');
      } catch (_) {
        throw Exception('not cached');
      }
      final session = await webdavSessionFor(widget.source);
      final p = await webdavCover(
          session: session,
          path: widget.path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop);
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (widget.source.isSftp) {
      try {
        final session = await sftpSessionFor(widget.source);
        final hasRaw = await sftpHasRawCache(session: session, path: widget.path);
        if (!hasRaw) throw Exception('no raw cache');
      } catch (_) {
        throw Exception('not cached');
      }
      final session = await sftpSessionFor(widget.source);
      final p = await sftpCover(
          session: session,
          path: widget.path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop);
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (widget.source.isBaidu) {
      try {
        final session = await baiduSessionFor(widget.source);
        final hasRaw = await baiduHasRawCache(session: session, path: widget.path);
        if (!hasRaw) throw Exception('no raw cache');
      } catch (_) {
        throw Exception('not cached');
      }
      final session = await baiduSessionFor(widget.source);
      final p = await baiduCover(
          session: session,
          path: widget.path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop);
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (widget.source.is115) {
      try {
        final session = await cloud115SessionFor(widget.source);
        final hasRaw =
            await cloud115HasRawCache(session: session, path: widget.path);
        if (!hasRaw) throw Exception('no raw cache');
      } catch (_) {
        throw Exception('not cached');
      }
      final session = await cloud115SessionFor(widget.source);
      final p = await cloud115Cover(
          session: session,
          path: widget.path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop);
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else if (widget.source.isQuark) {
      try {
        final session = await quarkSessionFor(widget.source);
        final hasRaw =
            await quarkHasRawCache(session: session, path: widget.path);
        if (!hasRaw) throw Exception('no raw cache');
      } catch (_) {
        throw Exception('not cached');
      }
      final session = await quarkSessionFor(widget.source);
      final p = await quarkCover(
          session: session,
          path: widget.path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop);
      return await rgbaToImage(p.rgba, p.width, p.height);
    } else {
      final p = await bookCover(
          path: widget.path, page: meta.coverPage, width: w, height: h, crop: crop);
      return await rgbaToImage(p.rgba, p.width, p.height);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_shouldSkipLoad) return _placeholder();
    if (_future == null) return _loading();

    return FutureBuilder<ui.Image>(
      future: _future,
      builder: (context, snap) {
        if (snap.hasData) {
          return RawImage(image: snap.data, fit: widget.fit);
        }
        if (snap.hasError) {
          return _placeholder();
        }
        return _loading();
      },
    );
  }

  Widget _loading() => Container(
    color: Colors.black26,
    child: const Center(
      child: SizedBox(
        width: 22, height: 22,
        child: CircularProgressIndicator(strokeWidth: 2),
      ),
    ),
  );

  Widget _placeholder() => Container(
    color: Colors.black26,
    child: Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.cloud_download_outlined,
              size: 36, color: Colors.lightBlueAccent.withAlpha(120)),
          const SizedBox(height: 4),
          const Text('未缓存', style: TextStyle(fontSize: 10, color: Colors.white38)),
        ],
      ),
    ),
  );
}

/// 漫画卡片：封面 + 标题 + 副标题，海报墙通用。
class ComicCard extends StatelessWidget {
  final BookSource source;
  final String path;
  final String title;
  final String? subtitle;
  final VoidCallback onTap;

  const ComicCard({
    super.key,
    required this.source,
    required this.path,
    required this.title,
    this.subtitle,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return Card(
      clipBehavior: Clip.antiAlias,
      elevation: 3,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
      child: InkWell(
        onTap: onTap,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(child: ComicCover(source: source, path: path)),
            Container(
              color: Colors.black45,
              padding: const EdgeInsets.fromLTRB(6, 5, 6, 6),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    title,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 12, height: 1.2),
                  ),
                  if (subtitle != null) ...[
                    const SizedBox(height: 2),
                    Text(subtitle!,
                        style: const TextStyle(fontSize: 10, color: Colors.white54)),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}
