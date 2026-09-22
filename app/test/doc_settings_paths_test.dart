import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

/// 文档里的「设置 → …」路径必须与 UI 中真实存在的分组 / 小节 / 入口一致。
///
/// 背景（2026-09-21）：README 与用户手册里曾出现 `设置 → 刮削`（UI 中不存在，
/// `ScrapePanel` 实际位于「书源与网络」分组下）等**失效路径**，用户按文档操作会找不到入口。
/// 本测试把"路径词表"固化成可执行检查，并在词表与代码漂移时同时报错。
///
/// 词表来源（全部由代码核对得出）：
/// - 分组：`app/lib/ui/home_page.dart` 的 `_settingsGroup(title: …)`
/// - 小节标题：`home_page.dart` 内 fontSize 16 / w600 的 Text
/// - 面板标题：`cache_manager.dart`、`update_panel.dart`、`backup_panel.dart` 等
/// - 行级文案与按钮：同上文件内的开关 / 下拉 / 按钮文案
///
/// 维护方式：UI 改名时本测试会先因"漂移守卫"失败，再更新词表即可。
void main() {
  const groups = <String>[
    "书源与网络",
    "同步与备份",
    "外观与布局",
    "关于与更新",
    "缓存与存储",
    "阅读",
  ];
  const sections = <String>["主题", "存储权限", "封面质量", "平板布局", "本地漫画", "自定义按键", "跨书源搜索", "远程书源", "阅读渲染宽度", "阅读默认"];
  const panels = <String>["关于与更新", "备份", "缓存管理"];
  const rowLabels = <String>["阅读渲染宽度", "自动转 CBZ", "下载通道", "关于与更新", "缓存管理", "直接流式", "阅读完成后删除整包（封面缓存保留）"];
  const buttons = <String>["重新刮削", "立即同步"];

  final validSegments = <String>{
    ...groups,
    ...sections,
    ...panels,
    ...rowLabels,
    ...buttons,
  };

  final repoRoot = Directory.current.parent.path; // app/ -> 仓库根
  final docs = <String, String>{
    'README.md': File('$repoRoot/README.md').readAsStringSync(),
    'docs/user-guide.md': File('$repoRoot/docs/user-guide.md').readAsStringSync(),
  };

  /// 提取 `设置 → A → B …`（反引号内），返回 (文件, 原始路径, 分段)。
  List<(String, String, List<String>)> extract() {
    final out = <(String, String, List<String>)>[];
    final re = RegExp(r'`设置\s*→\s*[^`]+`');
    docs.forEach((name, text) {
      for (final m in re.allMatches(text)) {
        final rawPath = m.group(0)!.replaceAll('`', '');
        final segments = rawPath
            .split('→')
            .map((s) => s.trim())
            .where((s) => s.isNotEmpty && s != '设置')
            .map((s) {
              // 去掉尾随的说明文字、书名号与括号内容
              var t = s.split(RegExp(r'[，,（(：:]')).first.trim();
              t = t.replaceAll(RegExp(r'^[「『]|[」』]$'), '').trim();
              return t;
            })
            .where((s) => s.isNotEmpty)
            .toList();
        out.add((name, rawPath, segments));
      }
    });
    return out;
  }

  test('文档中的「设置 → …」路径分段均在 UI 词表内', () {
    final paths = extract();
    expect(paths, isNotEmpty, reason: '文档中应至少出现一条设置路径');

    final bad = <String>[];
    for (final (file, raw, segments) in paths) {
      for (final seg in segments) {
        if (!validSegments.contains(seg)) {
          bad.add('$file: `$raw` 中的「$seg」在 UI 词表中不存在');
        }
      }
    }
    expect(bad, isEmpty,
        reason: '存在失效的设置路径（请到 app/lib/ui 核对真实入口名后再写入文档）:\n${bad.join('\n')}');
  });

  test('词表漂移守卫：每个标签仍能在代码中找到', () {
    final sources = <String>[
      'lib/ui/home_page.dart',
      'lib/ui/cache_manager.dart',
      'lib/ui/update_panel.dart',
      'lib/ui/backup_panel.dart',
      'lib/ui/scrape_panel.dart',
      'lib/ui/sync_panel.dart',
    ].where((p) => File(p).existsSync()).map((p) => File(p).readAsStringSync()).join('\n');

    final missing = <String>[
      ...groups,
      ...sections,
      ...panels,
      ...rowLabels,
      ...buttons,
    ].where((label) => !sources.contains(label)).toList();

    expect(missing, isEmpty,
        reason: '以下标签在 UI 代码中已找不到（可能被改名）：$missing —— 请更新本测试的词表与相关文档');
  });
}
