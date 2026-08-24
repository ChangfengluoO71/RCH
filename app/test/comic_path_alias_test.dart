import 'package:app/store/models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('normalizeComicPath：zip 家族剥离扩展名 → 与同名文件夹等价', () {
    expect(normalizeComicPath('/a/b.cbz'), '/a/b');
    expect(normalizeComicPath('/a/b.CBZ'), '/a/b'); // 大小写不敏感
    expect(normalizeComicPath('/a/b.zip'), '/a/b');
    expect(normalizeComicPath('/a/b.cbr'), '/a/b');
    expect(normalizeComicPath('/a/b.rar'), '/a/b');
    expect(normalizeComicPath('/a/b.cb7'), '/a/b');
    expect(normalizeComicPath('/a/b.7z'), '/a/b');
    expect(normalizeComicPath('/a/b.cbt'), '/a/b');
    expect(normalizeComicPath('/a/b.tar'), '/a/b');
  });

  test('normalizeComicPath：mobi 家族归一到 .mobi，其余格式不变', () {
    expect(normalizeComicPath('/a/b.azw'), '/a/b.mobi');
    expect(normalizeComicPath('/a/b.azw3'), '/a/b.mobi');
    expect(normalizeComicPath('/a/b.epub'), '/a/b.epub');
    expect(normalizeComicPath('/a/b.pdf'), '/a/b.pdf');
  });

  test('normalizeComicPath：路径分隔符和 Windows 盘符也归一化', () {
    expect(normalizeComicPath(r'F:\comic\日漫\book.cbz'), 'f:/comic/日漫/book');
    expect(normalizeComicPath(r'f:/comic/日漫/book.zip'), 'f:/comic/日漫/book');
  });

  test('bookKeyOf：文件夹 / zip / cbz 视为同一本，不同书 key 不同', () {
    expect(
      bookKeyOf('local', 's1', '/x/a'),
      bookKeyOf('local', 's1', '/x/a.cbz'),
    );
    expect(
      bookKeyOf('local', 's1', '/x/a.zip'),
      bookKeyOf('local', 's1', '/x/a.cbz'),
    );
    expect(
      bookKeyOf('local', 's1', '/x/a.zip'),
      isNot(bookKeyOf('local', 's1', '/x/b.zip')),
    );
    expect(
      bookKeyOf('local', 's1', '/x/a.epub'),
      isNot(bookKeyOf('local', 's1', '/x/a')),
    );
  });
}
