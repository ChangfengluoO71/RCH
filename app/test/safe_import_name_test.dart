// 回归测试：SAF/系统文件选择器导入时的文件名清洗。
import 'package:app/ui/home_page.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('safeImportedFileName', () {
    test('保留普通文件名与扩展名', () {
      expect(safeImportedFileName('one piece.cbz'), 'one piece.cbz');
      expect(safeImportedFileName('第01话.epub'), '第01话.epub');
    });

    test('去掉路径分隔符与非法字符', () {
      expect(safeImportedFileName(r'C:\dir\book.zip'), 'C_dir_book.zip');
      expect(safeImportedFileName('a:b*c?d"e<f>g|h.cbz'), 'a_b_c_d_e_f_g_h.cbz');
    });

    test('空名与危险占位回退', () {
      expect(safeImportedFileName(''), 'imported_comic.cbz');
      expect(safeImportedFileName('   '), 'imported_comic.cbz');
      expect(safeImportedFileName('..'), 'imported_comic.cbz');
    });
  });
}
