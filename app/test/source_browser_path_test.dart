import 'package:app/ui/source_browser.dart' show sourceBrowserEntryPath;
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('local Windows entries keep their drive-rooted filesystem path', () {
    const entryPath = r'F:\comic\[artist] title';

    final result = sourceBrowserEntryPath(
      isLocalFs: true,
      usesOpaqueProviderPaths: false,
      currentLogicalPath: '/F:/comic',
      entryName: '[artist] title',
      entryPath: entryPath,
    );

    expect(result, entryPath);
  });

  test('remote entries remain rooted provider paths', () {
    final result = sourceBrowserEntryPath(
      isLocalFs: false,
      usesOpaqueProviderPaths: false,
      currentLogicalPath: '/library',
      entryName: 'book.cbz',
      entryPath: 'library/book.cbz',
    );

    expect(result, '/library/book.cbz');
  });

  test('opaque providers build logical paths from the current directory', () {
    final result = sourceBrowserEntryPath(
      isLocalFs: false,
      usesOpaqueProviderPaths: true,
      currentLogicalPath: '/library',
      entryName: 'book.cbz',
      entryPath: 'provider-entry-id',
    );

    expect(result, '/library/book.cbz');
  });
}
