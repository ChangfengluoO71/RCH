// 数据模型:书源与阅读记录(JSON 持久化)。

import 'package:flutter/services.dart';

/// 书源:本地目录或 WebDAV。
class BookSource {
  final String id;
  final String type; // 'local' | 'webdav'
  String name;
  String path; // local: 目录路径;webdav: 初始浏览路径
  String? url;
  String? username;
  String? password;
  String note; // 用户备注
  String capabilityLabel; // "local" | "webdav_range" | "webdav_norange"

  BookSource({
    required this.id,
    required this.type,
    required this.name,
    this.path = '',
    this.url,
    this.username,
    this.password,
    this.note = '',
    this.capabilityLabel = '',
  });

  bool get isWebDav => type == 'webdav';

  /// 能力标记的显示颜色。
  /// 🟢 本地/NAS  🟡 WebDAV(Range)  🔴 WebDAV(无Range)
  ({String emoji, String label}) get capabilityDisplay {
    if (type == 'local') return (emoji: '\u{1F7E2}', label: '本地');
    if (capabilityLabel == 'local') return (emoji: '\u{1F7E2}', label: 'WebDAV 高速');
    if (capabilityLabel == 'webdav_range') return (emoji: '\u{1F7E1}', label: 'WebDAV 远程');
    return (emoji: '\u{1F534}', label: 'WebDAV 无Range');
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        'type': type,
        'name': name,
        'path': path,
        if (url != null) 'url': url,
        if (username != null) 'username': username,
        if (password != null) 'password': password,
        'note': note,
        if (capabilityLabel.isNotEmpty) 'capabilityLabel': capabilityLabel,
      };

  factory BookSource.fromJson(Map<String, dynamic> j) => BookSource(
        id: j['id'] as String,
        type: j['type'] as String,
        name: j['name'] as String,
        path: j['path'] as String? ?? '/',
        url: j['url'] as String?,
        username: j['username'] as String?,
        password: j['password'] as String?,
        note: (j['note'] as String?) ?? '',
        capabilityLabel: (j['capabilityLabel'] as String?) ?? '',
      );
}

/// 阅读记录:最近/最多阅读、阅读进度。
class ReadRecord {
  final String key; // 唯一键:type|sourceId|path
  final String path;
  final String sourceId;
  final String sourceType;
  final String title;
  int lastPage;
  int readCount;
  int lastReadAt; // 毫秒时间戳

  ReadRecord({
    required this.key,
    required this.sourceId,
    required this.sourceType,
    required this.path,
    required this.title,
    this.lastPage = 0,
    this.readCount = 0,
    this.lastReadAt = 0,
  });

  Map<String, dynamic> toJson() => {
        'key': key,
        'sourceId': sourceId,
        'sourceType': sourceType,
        'path': path,
        'title': title,
        'lastPage': lastPage,
        'readCount': readCount,
        'lastReadAt': lastReadAt,
      };

  factory ReadRecord.fromJson(Map<String, dynamic> j) => ReadRecord(
        key: j['key'] as String,
        sourceId: j['sourceId'] as String,
        sourceType: j['sourceType'] as String,
        path: j['path'] as String,
        title: j['title'] as String,
        lastPage: j['lastPage'] as int? ?? 0,
        readCount: j['readCount'] as int? ?? 0,
        lastReadAt: j['lastReadAt'] as int? ?? 0,
      );
}

/// 封面质量(影响扫描速度与清晰度)。
enum CoverQuality { low, medium, high }

/// 阅读模式。
enum ReadMode {
  /// 日漫:从右到左翻页。
  manga('日漫'),
  /// 美漫:从左到右翻页。
  comic('美漫'),
  /// 条漫:垂直连续滚动。
  webtoon('条漫');

  const ReadMode(this.label);
  final String label;
}

/// 双页拼接模式。
enum DualPageMode {
  /// 单页显示。
  off('关'),
  /// 固定双页拼接。
  force('开');

  const DualPageMode(this.label);
  final String label;
}

extension CoverQualitySize on CoverQuality {
  /// 封面缩略图的目标 (宽,高) 像素。
  (int, int) get size => switch (this) {
        CoverQuality.low => (170, 240),
        CoverQuality.medium => (340, 480),
        CoverQuality.high => (510, 720),
      };

  String get label => switch (this) {
        CoverQuality.low => '低(最快)',
        CoverQuality.medium => '中(默认)',
        CoverQuality.high => '高(最清晰)',
      };
}

/// 自定义键盘绑定(5 个动作,只存 keyId;键盘 only,不含鼠标组合键)。
class KeyBinds {
  int forward;   // 前进
  int back;      // 后退
  int zoomIn;    // 放大
  int zoomOut;   // 缩小
  int zoomReset; // 缩放还原

  KeyBinds({
    int? forward,
    int? back,
    int? zoomIn,
    int? zoomOut,
    int? zoomReset,
  })  : forward = forward ?? LogicalKeyboardKey.arrowRight.keyId,
        back = back ?? LogicalKeyboardKey.arrowLeft.keyId,
        zoomIn = zoomIn ?? LogicalKeyboardKey.equal.keyId,
        zoomOut = zoomOut ?? LogicalKeyboardKey.minus.keyId,
        zoomReset = zoomReset ?? LogicalKeyboardKey.digit0.keyId;

  LogicalKeyboardKey get forwardKey => LogicalKeyboardKey.findKeyByKeyId(forward) ?? LogicalKeyboardKey.arrowRight;
  LogicalKeyboardKey get backKey => LogicalKeyboardKey.findKeyByKeyId(back) ?? LogicalKeyboardKey.arrowLeft;
  LogicalKeyboardKey get zoomInKey => LogicalKeyboardKey.findKeyByKeyId(zoomIn) ?? LogicalKeyboardKey.equal;
  LogicalKeyboardKey get zoomOutKey => LogicalKeyboardKey.findKeyByKeyId(zoomOut) ?? LogicalKeyboardKey.minus;
  LogicalKeyboardKey get zoomResetKey => LogicalKeyboardKey.findKeyByKeyId(zoomReset) ?? LogicalKeyboardKey.digit0;

  Map<String, dynamic> toJson() => {
        'forward': forward,
        'back': back,
        'zoomIn': zoomIn,
        'zoomOut': zoomOut,
        'zoomReset': zoomReset,
      };

  factory KeyBinds.fromJson(Map<String, dynamic>? j) => KeyBinds(
        forward: j?['forward'] as int?,
        back: j?['back'] as int?,
        zoomIn: j?['zoomIn'] as int?,
        zoomOut: j?['zoomOut'] as int?,
        zoomReset: j?['zoomReset'] as int?,
      );
}

/// 应用设置。
class AppSettings {
  CoverQuality coverQuality;
  String themeMode; // 'light' | 'dark'
  ReadMode readMode; // 阅读模式:日漫/美漫/条漫
  bool invertTap; // 日漫模式下点击区是否反向
  DualPageMode dualPageMode; // 双页拼接模式
  int dualPageGap; // 双页拼接中间缝隙像素
  bool skipFrontCover; // 首页不拼
  KeyBinds keys; // 自定义按键

  AppSettings({
    this.coverQuality = CoverQuality.medium,
    this.themeMode = 'dark',
    this.readMode = ReadMode.manga,
    this.invertTap = false,
    this.dualPageMode = DualPageMode.off,
    this.dualPageGap = 0,
    this.skipFrontCover = true,
    KeyBinds? keys,
  }) : keys = keys ?? KeyBinds();

  Map<String, dynamic> toJson() => {
        'coverQuality': coverQuality.name,
        'themeMode': themeMode,
        'readMode': readMode.name,
        'invertTap': invertTap,
        'dualPageMode': dualPageMode.name,
        'dualPageGap': dualPageGap,
        'skipFrontCover': skipFrontCover,
        'keys': keys.toJson(),
      };

  factory AppSettings.fromJson(Map<String, dynamic> j) => AppSettings(
        coverQuality: CoverQuality.values.firstWhere(
          (q) => q.name == j['coverQuality'],
          orElse: () => CoverQuality.medium,
        ),
        themeMode: (j['themeMode'] as String?) ?? 'dark',
        readMode: ReadMode.values.firstWhere(
          (r) => r.name == j['readMode'],
          orElse: () => ReadMode.manga,
        ),
        invertTap: (j['invertTap'] as bool?) ?? false,
        dualPageMode: DualPageMode.values.firstWhere(
          (d) => d.name == j['dualPageMode'],
          orElse: () => DualPageMode.off,
        ),
        dualPageGap: (j['dualPageGap'] as int?) ?? 0,
        skipFrontCover: (j['skipFrontCover'] as bool?) ?? true,
        keys: KeyBinds.fromJson(j['keys'] as Map<String, dynamic>?),
      );
}

/// 一本书的元数据(自定义封面 / 标签 / 简介 / 感想)。
class BookMeta {
  final String key; // type|sourceId|path

  int coverPage;
  double? cropX, cropY, cropW, cropH; // 裁剪区域(相对 0-1),null=整页
  List<String> tags;
  // 元数据标签(作者/类别/系列,智能扫描用)
  String author;   // 作者
  String genre;    // 类别
  String series;   // 系列
  String summary; // 简介
  String comment; // 感想
  String title; // 标题(默认原文件名)
  String chineseTitle; // 中文标题

  BookMeta({
    required this.key,
    this.coverPage = 0,
    this.cropX,
    this.cropY,
    this.cropW,
    this.cropH,
    List<String>? tags,
    this.author = '',
    this.genre = '',
    this.series = '',
    this.summary = '',
    this.comment = '',
    this.title = '',
    this.chineseTitle = '',
  }) : tags = tags ?? [];

  bool get hasCrop => cropX != null;

  List<String> get metaTags => [if (author.isNotEmpty) author, if (genre.isNotEmpty) genre, if (series.isNotEmpty) series];

  Map<String, dynamic> toJson() => {
        'key': key,
        'coverPage': coverPage,
        if (cropX != null) 'cropX': cropX,
        if (cropY != null) 'cropY': cropY,
        if (cropW != null) 'cropW': cropW,
        if (cropH != null) 'cropH': cropH,
        'tags': tags,
        'author': author,
        'genre': genre,
        'series': series,
        'title': title,
        'chineseTitle': chineseTitle,
        'summary': summary,
        'comment': comment,
      };

  factory BookMeta.fromJson(Map<String, dynamic> j) => BookMeta(
        key: j['key'] as String,
        coverPage: j['coverPage'] as int? ?? 0,
        cropX: (j['cropX'] as num?)?.toDouble(),
        cropY: (j['cropY'] as num?)?.toDouble(),
        cropW: (j['cropW'] as num?)?.toDouble(),
        cropH: (j['cropH'] as num?)?.toDouble(),
        tags: (j['tags'] as List?)?.map((e) => '$e').toList() ?? [],
        author: (j['author'] as String?) ?? '',
        genre: (j['genre'] as String?) ?? '',
        series: (j['series'] as String?) ?? '',
        title: (j['title'] as String?) ?? '',
        chineseTitle: (j['chineseTitle'] as String?) ?? '',
        summary: (j['summary'] as String?) ?? '',
        comment: (j['comment'] as String?) ?? '',
      );
}

// ============================================================
// ADR-017: 标签独立建模 — Tag 实体 + BookTag 关联
// ============================================================

/// 标签实体: 独立于 BookMeta 存在（ADR-017）。
/// 即使没有任何漫画关联，标签也能存在于补全列表中。
class Tag {
  final String id; // 唯一标识（用标签名的 stable hash 或时间戳生成）
  String name;
  final int createdAt; // 毫秒时间戳

  Tag({required this.id, required this.name, int? createdAt})
    : createdAt = createdAt ?? DateTime.now().millisecondsSinceEpoch;

  Map<String, dynamic> toJson() => {'id': id, 'name': name, 'createdAt': createdAt};

  factory Tag.fromJson(Map<String, dynamic> j) => Tag(
    id: j['id'] as String,
    name: j['name'] as String,
    createdAt: (j['createdAt'] as int?) ?? 0,
  );
}

/// 漫画-标签关联: book_key ↔ tag_id（ADR-017）。
class BookTag {
  final String bookKey; // type|sourceId|path
  final String tagId;

  const BookTag({required this.bookKey, required this.tagId});

  /// Set 去重依赖：相同 bookKey + tagId 视为同一关联。
  @override
  bool operator ==(Object other) =>
      other is BookTag && other.bookKey == bookKey && other.tagId == tagId;

  @override
  int get hashCode => Object.hash(bookKey, tagId);

  Map<String, dynamic> toJson() => {'bookKey': bookKey, 'tagId': tagId};

  factory BookTag.fromJson(Map<String, dynamic> j) => BookTag(
    bookKey: j['bookKey'] as String,
    tagId: j['tagId'] as String,
  );
}

/// library.json 顶层包装（ADR-016/017），含版本号用于向后兼容数据迁移。
class LibraryData {
  int version;
  List<BookSource> sources;
  Map<String, ReadRecord> records;
  Map<String, BookMeta> metas;
  AppSettings settings;

  LibraryData({
    this.version = 1,
    List<BookSource>? sources,
    Map<String, ReadRecord>? records,
    Map<String, BookMeta>? metas,
    AppSettings? settings,
  })  : sources = sources ?? [],
        records = records ?? {},
        metas = metas ?? {},
        settings = settings ?? AppSettings();
}
