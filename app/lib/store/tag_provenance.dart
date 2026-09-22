import 'package:flutter/material.dart';

/// 标签来源：决定分色方框的颜色与分组。
///
/// 判定依据是**既有前缀命名约定**（项目里 `TagRepository.isVisibleInTagManager` 已在用
/// `resource:` / `sequence:` / `publication:` 等英文前缀），E 站导入沿用中文前缀：
/// `源:e站`、`女性:巨乳`、`作者:朝凪`……
enum TagSource {
  /// 用户自建（无前缀）。
  user,

  /// E 站导入（中文命名空间前缀，或来源标记 `源:e站`）。
  ehImport,

  /// 刮削生成（英文内部前缀）。
  scraper,
}

/// E 站导入使用的中文命名空间前缀（与 Rust 侧 `eh_import::namespace_prefix` 对齐）。
const List<String> kEhTagPrefixes = [
  '女性:',
  '男性:',
  '混合:',
  '属性:',
  '重分类:',
  '语言:',
  '作者:',
  '社团:',
  '原作:',
  '角色:',
];

/// 来源标记本身（详情页置顶那一行；点击=隐藏该书 E 站导入标签）。
const String kEhSourceTag = '源:e站';

/// 刮削生成的英文内部前缀（与 `TagRepository.isVisibleInTagManager` 的清单一致）。
const List<String> _kScraperPrefixes = [
  'resource:',
  'sequence:',
  'publication:',
  'release:',
  'release-group:',
  'release_group:',
  'provider:',
  'source:',
  'language:',
  'translation:',
  'translation-method:',
  'translation_method:',
  'edition:',
  'censorship:',
  'color:',
  'completeness:',
  'medium:',
  'scan:',
  'tag:',
  '汉化组：',
  '汉化组:',
];

/// 解析单个标签的来源。
TagSource tagSourceOf(String name) {
  final t = name.trim();
  if (t == kEhSourceTag) return TagSource.ehImport;
  for (final p in kEhTagPrefixes) {
    if (t.startsWith(p)) return TagSource.ehImport;
  }
  final lower = t.toLowerCase();
  for (final p in _kScraperPrefixes) {
    if (lower.startsWith(p)) return TagSource.scraper;
  }
  return TagSource.user;
}

/// 该书是否含任何 E 站导入标签（含来源标记）。
bool hasEhImportedTags(Iterable<String> tags) =>
    tags.any((t) => tagSourceOf(t) == TagSource.ehImport);

/// 去掉命名空间前缀，只显示值（`女性:巨乳` → `巨乳`）；无前缀则原样返回。
String tagDisplayName(String name) {
  final t = name.trim();
  final i = t.indexOf(':');
  if (i <= 0) return t;
  final prefix = t.substring(0, i);
  return kEhTagPrefixes.contains('$prefix:') ? t.substring(i + 1) : t;
}

/// 命名空间前缀（`女性:巨乳` → `女性`）；无前缀返回 null。
String? tagNamespaceLabel(String name) {
  final t = name.trim();
  final i = t.indexOf(':');
  if (i <= 0) return null;
  final prefix = t.substring(0, i);
  return kEhTagPrefixes.contains('$prefix:') || _kScraperPrefixes.contains('$prefix:')
      ? prefix
      : null;
}

/// 按来源取分色（每个来源一个专属色，一眼可分）。
Color tagSourceColor(TagSource source, ColorScheme scheme) => switch (source) {
  TagSource.user => scheme.onSurfaceVariant,
  TagSource.ehImport => const Color(0xFF8E3B46), // E 站主题暗红，与用户/刮削明显区分
  TagSource.scraper => scheme.tertiary,
};

/// 来源的中文名（提示气泡用）。
String tagSourceLabel(TagSource source) => switch (source) {
  TagSource.user => '自建',
  TagSource.ehImport => 'E 站导入',
  TagSource.scraper => '刮削生成',
};

/// 一个标签方框：按来源分色（边框 + 淡底），带命名空间前缀的弱化显示。
///
/// `compact=true` 时用于信息区（不可删）；否则可删（带 × 按钮）。
class TagBox extends StatelessWidget {
  const TagBox({
    super.key,
    required this.name,
    this.onDeleted,
    this.tooltip,
  });

  final String name;
  final VoidCallback? onDeleted;
  final String? tooltip;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final source = tagSourceOf(name);
    final color = tagSourceColor(source, scheme);
    final ns = tagNamespaceLabel(name);

    final body = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (ns != null) ...[
          Text(
            '$ns:',
            style: TextStyle(fontSize: 11, color: color.withValues(alpha: 0.75)),
          ),
        ],
        Text(
          tagDisplayName(name),
          style: TextStyle(fontSize: 12, color: color, fontWeight: FontWeight.w500),
        ),
        if (onDeleted != null) ...[
          const SizedBox(width: 4),
          InkWell(
            onTap: onDeleted,
            child: Icon(Icons.close, size: 13, color: color.withValues(alpha: 0.8)),
          ),
        ],
      ],
    );

    final box = Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.10),
        borderRadius: BorderRadius.circular(6),
        border: Border.all(color: color.withValues(alpha: 0.45)),
      ),
      child: body,
    );
    return tooltip == null ? box : Tooltip(message: tooltip!, child: box);
  }
}
