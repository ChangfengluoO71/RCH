// 资料库目录数据模型（Phase 6.1）。
//
// Flutter 只消费 Rust 侧语义 DTO（device → source → 可用性），
// 不在此处重新判断 remote_only / fingerprint / 设备归属。

import 'package:app/src/rust/api/library.dart' as frb;
import 'package:flutter/foundation.dart';

class LibraryCatalogStore extends ChangeNotifier {
  LibraryCatalogStore._();
  static final LibraryCatalogStore instance = LibraryCatalogStore._();

  List<frb.SourceTreeNodeDto> devices = [];
  bool loaded = false;

  /// 加载设备 → 书源树（本机逻辑书源在前，远端按设备分组）。
  Future<void> loadTree() async {
    try {
      devices = await frb.dbSourceTree();
      loaded = true;
    } catch (_) {
      devices = [];
      loaded = false;
    }
    notifyListeners();
  }

  /// 跨设备资料库搜索（分页）。
  Future<List<frb.BookSearchDto>> searchBooks({
    String query = '',
    List<String> tags = const [],
    bool includeRemote = true,
    int limit = 100,
    int offset = 0,
  }) =>
      frb.dbSearchBooks(
        query: query,
        tags: tags,
        includeRemote: includeRemote,
        limit: limit,
        offset: offset,
      );

  /// 某书源下的漫画（分页；书源节点展开时懒加载）。
  Future<List<frb.BookSearchDto>> sourceBooks({
    required String sourceId,
    String query = '',
    List<String> tags = const [],
    int limit = 100,
    int offset = 0,
  }) =>
      frb.dbSourceBooks(
        sourceId: sourceId,
        query: query,
        tags: tags,
        limit: limit,
        offset: offset,
      );

  /// 三状态图标映射（语义由 Rust DTO 给出，这里只做展示）。
  static String statusEmoji(String status) => switch (status) {
        'read' => '🟢',
        'needs_network' => '🟡',
        _ => '⚪',
      };

  static String statusLabel(String status) => switch (status) {
        'read' => '可阅读',
        'needs_network' => '需连接',
        _ => '仅索引',
      };
}
