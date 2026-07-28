import 'dart:ui' as ui;

import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/common.dart';
import 'package:app/ui/opener.dart';
import 'package:flutter/material.dart';

/// 漫画封面:统一本地 / WebDAV 来源,带全局内存缓存。
/// WebDAV 封面采用懒加载策略:未打开过的漫画不主动加载封面(避免大量网络请求),
/// 仅当用户已阅读(有缓存记录)时**并且有本地缓存可秒出**时才加载封面。
/// 如果已阅读但本地缓存不存在（如封面缓存被清理），则不尝试远程加载，
/// 直接显示"未缓存"（与未阅读过的漫画一致），避免网络慢导致一直转圈。
class ComicCover extends StatelessWidget {
  final BookSource source;
  final String path;
  final BoxFit fit;
  /// 强制加载封面(即使用户从未打开过此漫画)。用于手动触发加载。
  final bool force;

  const ComicCover({
    super.key,
    required this.source,
    required this.path,
    this.fit = BoxFit.cover,
    this.force = false,
  });

  static final Map<String, Future<ui.Image>> _cache = {};

  /// 清空封面内存/磁盘缓存(质量切换或手动清理时调用)。
  static void clear() {
    _cache.clear();
  }

  /// 移除指定封面的缓存(用于失败后重试)。
  static void evict(String key) {
    _cache.remove(key);
  }

  /// 移除某个源+路径的所有封面缓存(用于阅读后强制刷新)。
  static void evictAll(String sourceId, String path) {
    _cache.removeWhere((k, _) => k.startsWith('$sourceId|$path'));
  }

  String get _cacheKey {
    final store = LibraryStore.instance;
    final q = store.settings.coverQuality;
    final meta = store.metaOf(source, path);
    return '${source.id}|$path|${q.name}|${meta.coverPage}'
        '|${meta.cropX},${meta.cropY},${meta.cropW},${meta.cropH}';
  }

  /// 本地漫画始终加载；WebDAV 未阅读过→不加载；强制模式→总是加载。
  bool get _shouldSkipLoad {
    if (!source.isWebDav) return false; // 本地始终加载
    if (force) return false; // 强制加载
    // WebDAV: 仅当有阅读记录时才尝试加载
    final store = LibraryStore.instance;
    final key = '${source.type}|${source.id}|$path';
    return !store.records.containsKey(key);
  }

  /// 异步检查 WebDAV 漫画是否有 raw/ 本地缓存。
  /// 有 → 从本地秒出封面；没有 → 不尝试远程加载，直接显示"未缓存"。
  Future<bool> _hasRawCache() async {
    if (!source.isWebDav) return true;
    try {
      // 调 Rust 端检测 raw/ 本地缓存路径是否存在
      return await webdavHasRawCache(
        session: (await webdavSessionFor(source)),
        path: path,
      );
    } catch (_) {
      return false;
    }
  }

  Future<ui.Image> _load() async {
    final store = LibraryStore.instance;
    final q = store.settings.coverQuality;
    final (w, h) = q.size;
    final meta = store.metaOf(source, path);
    final crop = meta.hasCrop
        ? CropRect(x: meta.cropX!, y: meta.cropY!, w: meta.cropW!, h: meta.cropH!)
        : null;
    final key = _cacheKey;
    // 内存缓存命中
    final existing = _cache[key];
    if (existing != null) return existing;

    if (source.isWebDav) {
      final session = await webdavSessionFor(source);
      final p = await webdavCover(
          session: session,
          path: path,
          page: meta.coverPage,
          width: w,
          height: h,
          crop: crop);
      final img = await rgbaToImage(p.rgba, p.width, p.height);
      _cache[key] = Future.value(img);
      return img;
    } else {
      final p = await bookCover(
          path: path, page: meta.coverPage, width: w, height: h, crop: crop);
      final img = await rgbaToImage(p.rgba, p.width, p.height);
      _cache[key] = Future.value(img);
      return img;
    }
  }

  @override
  Widget build(BuildContext context) {
    // WebDAV 懒加载: 未阅读过的漫画显示占位图标
    if (_shouldSkipLoad) {
      return _placeholder();
    }

    return FutureBuilder<bool>(
      future: _hasRawCache(),
      builder: (context, snap) {
        // 正在检查本地缓存
        if (!snap.hasData) {
          return Container(color: Colors.black26, child: const Center(
            child: SizedBox(width: 22, height: 22, child: CircularProgressIndicator(strokeWidth: 2)),
          ));
        }
        // 无本地缓存 → 不尝试远程加载，直接显示"未缓存"
        if (snap.data != true) {
          return _placeholder();
        }
        // 有本地缓存 → 加载封面
        return _loadCover();
      },
    );
  }

  Widget _placeholder() {
    return Container(
      color: Colors.black26,
      child: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.cloud_download_outlined,
                size: 36, color: Colors.lightBlueAccent.withAlpha(120)),
            const SizedBox(height: 4),
            Text('未缓存', style: TextStyle(fontSize: 10, color: Colors.white38)),
          ],
        ),
      ),
    );
  }

  Widget _loadCover() {
    return FutureBuilder<ui.Image>(
      future: _load(),
      builder: (context, snap) {
        if (snap.hasData) {
          return RawImage(image: snap.data, fit: fit);
        }
        if (snap.hasError) {
          return _placeholder();
        }
        // 加载中（本地文件一般是秒出）
        return Container(color: Colors.black26, child: const Center(
          child: SizedBox(width: 22, height: 22, child: CircularProgressIndicator(strokeWidth: 2)),
        ));
      },
    );
  }
}

/// 漫画卡片:封面 + 标题 + 副标题,海报墙通用。
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
