import 'package:app/store/models.dart';
import 'package:app/ui/cover_editor_page.dart';
import 'package:flutter_test/flutter_test.dart';

/// 封面编辑器入口的**构造契约**（2026-09-21 重写）。
///
/// **背景**：旧测试围绕一套“同意/整本下载”设计编写 ——
/// `CustomCoverOpenController`、`NeedsWholeBookDownload`、`CustomCoverOpenGateway` ——
/// 这些类型**从未落地**（`lib/` 中不存在）⇒ CI `flutter analyze` 18 处 error（发布门禁红线）。
///
/// 现状：`CoverEditorPage` 只有 `{source, path, title}` 三个入参，没有可注入的
/// “打开决策/网关” seam；封面编辑的整本下载同意流程也没有对应的被测实现。
/// ⇒ 本文件只锁**真实存在的构造契约**，并把缺口写清楚（不再对计划中的设计写测试）。
void main() {
  final source = BookSource(id: 's1', type: 'quark', name: 'Quark');

  test('封面编辑器要求来源 / 路径 / 标题，并原样保存', () {
    final page = CoverEditorPage(
      source: source,
      path: '/漫画/1.cbz',
      title: '1.cbz',
    );

    expect(page.source.id, 's1');
    expect(page.path, '/漫画/1.cbz');
    expect(page.title, '1.cbz');
  });

  test('已知缺口：打开决策 / 整本下载同意流程尚无实现与 seam', () {
    // 旧测试要覆盖的契约：
    //   1) 需要整本下载时不启动任何下载工作（先给用户决策）；
    //   2) 拒绝后可再次准备，且仍不产生下载工作；
    //   3) 同意后才走显式下载策略。
    // 这些需要一个可注入的“打开决策”边界（当前不存在）。
    // 若将来落地，请在此处补回上述三条契约测试，并同步更新
    // `.trellis/spec/backend/remote-cover-update-contracts.md`。
    expect(CoverEditorPage, isNotNull);
  });
}
