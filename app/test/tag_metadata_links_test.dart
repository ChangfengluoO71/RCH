import 'package:app/repository/tag_repository.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/book_detail_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('resource/generated tags follow the user-facing visibility policy', () {
    expect(TagRepository.isVisibleInTagManager('Chinese'), isTrue);
    expect(TagRepository.isVisibleInTagManager('高清'), isTrue);
    expect(TagRepository.isVisibleInTagManager('无修正'), isTrue);
    expect(TagRepository.isVisibleInTagManager('中文翻译'), isFalse);
    expect(TagRepository.isVisibleInTagManager('数字版'), isFalse);
    expect(TagRepository.isVisibleInTagManager('release-group:示例汉化组'), isFalse);
    for (final internalName in const [
      'translation:translated',
      'edition:digital',
      'censorship:uncensored',
      'color:full_color',
      'tag:complete',
    ]) {
      expect(
        TagRepository.isVisibleInTagManager(internalName),
        isFalse,
        reason: '内部生成标签不应在启动/归一化期间闪现：$internalName',
      );
    }
    expect(TagRepository.isVisibleInTagManager('已读'), isTrue);
    expect(TagRepository.isVisibleInTagManager('作者甲'), isTrue);
    expect(
      TagRepository.isVisibleInTagManager('合集', metadataNames: const {'合集'}),
      isTrue,
    );
  });

  test('archive aliases share tag links after book-key normalization', () {
    final repo = TagRepository.instance;
    const rawKey = r'local|tag-regression|C:\comics\reader.cbz';
    const normalizedKey = r'local|tag-regression|C:\comics\reader';

    repo.link(rawKey, '已读');
    addTearDown(() => repo.unlink(rawKey, '已读'));

    expect(repo.tagsForBook(normalizedKey), contains('已读'));
  });

  test(
    'metadata fields are queryable through the same tag projection',
    () async {
      final repo = TagRepository.instance;
      final key = bookKeyOf(
        'local',
        'tag-regression-meta',
        r'C:\comics\meta.cbz',
      );

      await repo.syncMetadataLinks([
        BookMeta(key: key, author: '测试作者', genre: '漫画', series: '测试系列'),
      ], persist: false);

      expect(
        repo.tagsForBook(key),
        containsAll(<String>['测试作者', '漫画', '测试系列']),
      );
    },
  );

  testWidgets(
    'detail projection refreshes after metadata and read-tag changes',
    (tester) async {
      final store = LibraryStore.instance;
      final repo = TagRepository.instance;
      final source = BookSource(
        id: 'tag-regression-ui',
        type: 'local',
        name: 'tag regression',
        path: r'C:\comics',
      );
      final path = r'C:\comics\detail.cbz';
      final meta = store.metaOf(source, path)
        ..author = '界面作者'
        ..series = '界面系列';
      await repo.syncMetadataLinks([meta], persist: false);
      addTearDown(() {
        repo.setBookTags(meta.key, const []);
        store.metas.remove(meta.key);
      });

      await tester.pumpWidget(
        MaterialApp(
          home: BookDetailPage(source: source, path: path, title: 'detail.cbz'),
        ),
      );
      await tester.pump();

      expect(find.text('界面作者'), findsWidgets);
      expect(find.text('界面系列'), findsWidgets);

      repo.link(meta.key, '已读');
      await tester.pump();
      expect(find.text('已读'), findsWidgets);
    },
  );
  test(
    'stale book cleanup prunes orphan tags but preserves shared tags',
    () async {
      final repo = TagRepository.instance;
      const stale = r'local|tag-prune|stale.cbz';
      const live = r'local|tag-prune|live.cbz';
      const orphan = 'cleanup-only-tag';
      const shared = 'cleanup-shared-tag';

      repo.link(stale, orphan);
      repo.link(stale, shared);
      repo.link(live, shared);
      addTearDown(() {
        repo.removeBookTagsAndPrune(stale, persist: false);
        repo.removeBookTagsAndPrune(live, persist: false);
      });

      await repo.removeBookTagsAndPrune(stale, persist: false);

      expect(repo.tagsForBook(stale), isEmpty);
      expect(repo.allNames(), isNot(contains(orphan)));
      expect(repo.allNames(), contains(shared));
      expect(repo.tagsForBook(live), contains(shared));
    },
  );
}
