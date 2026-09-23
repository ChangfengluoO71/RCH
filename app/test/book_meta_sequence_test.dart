import 'package:app/repository/book_repository.dart';
import 'package:app/src/rust/api/db.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/book_detail_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// 卷/话（章节/部号）从 SQLite 到内存模型的回归。
///
/// 背景：第 129/130 轮把 `semantic.volume/chapter` 物化落库并加进 Dart 模型，但读方向
/// （DTO→BookMeta、`fromJson`、`_copyMetaWithKey` / `_mergeMeta`）漏了 —— SQLite 里即便
/// 有值，Dart 内存里也永远是空串，于是「章节/部号还是不显示」。本文件钉住 Dart 侧出口。
void main() {
  test('BookMetaDto → BookMeta 必须带上 volume/chapter', () {
    final meta = BookRepository.bookMetaFromDto(
      const BookMetaDto(
        key: 'local|1|/a/1.pdf',
        coverPage: 0,
        author: '',
        genre: '',
        series: '',
        title: '金牌得主',
        chineseTitle: '',
        summary: '',
        comment: '',
        rotations: '{}',
        volume: '1',
        chapter: '2',
      ),
    );
    expect(meta.volume, '1');
    expect(meta.chapter, '2');
  });

  test('BookMeta JSON 往返保留 volume/chapter', () {
    final m = BookMeta(key: 'local|1|/a/1.pdf')
      ..volume = '1'
      ..chapter = '2';

    final restored = BookMeta.fromJson(m.toJson());
    expect(restored.volume, '1');
    expect(restored.chapter, '2');
  });

  test('缺省/旧数据不编造号码（空串=没有解析到）', () {
    final m = BookMeta.fromJson({'key': 'x'});
    expect(m.volume, isEmpty);
    expect(m.chapter, isEmpty);
  });

  test('显示口径：话优先于卷，空号不显示也不默认成 1', () {
    expect(sequenceNumberOf(chapter: '', volume: ''), isEmpty);
    expect(sequenceNumberOf(chapter: '', volume: '1'), '1');
    expect(sequenceNumberOf(chapter: '2', volume: '1'), '2');
    expect(sequenceNumberOf(chapter: ' 2 ', volume: '1'), '2');

    // 三个显示出口共用同一拼接规则：无号 ⇒ 原样（不追加 " 1"）。
    expect(titleWithSequence('金牌得主', ''), '金牌得主');
    expect(titleWithSequence('金牌得主', '32'), '金牌得主 32');
  });

  // 真实渲染证据（不是"副本测试"）：详情页标题确实带号码，且不写回规范标题。
  // 复用 overflow_repro_test.dart 的做法：该页无需 RustLib.init 即可渲染。
  testWidgets('详情页标题显示「作品名 话号」，不改写规范标题，缺号不默认', (tester) async {
    final source = BookSource(
      id: 'local_sequence_render',
      type: 'local',
      name: 't',
      path: r'C:\comics',
    );
    const path = r'C:\comics\1.pdf';
    final meta = LibraryStore.instance.metaOf(source, path)
      ..title = '金牌得主'
      ..chapter = '32';

    await tester.pumpWidget(
      MaterialApp(
        home: BookDetailPage(source: source, path: path, title: '1.pdf'),
      ),
    );
    await tester.pump();

    expect(find.text('金牌得主 32'), findsOneWidget);
    expect(meta.title, '金牌得主', reason: '显示层拼号码不得写回规范标题');

    // 缺号：标题回到原样，且**不**默认成「1」。
    meta.chapter = '';
    meta.volume = '';
    await tester.pump();
    expect(find.text('金牌得主'), findsOneWidget);
    expect(find.text('金牌得主 1'), findsNothing);
  });
}
