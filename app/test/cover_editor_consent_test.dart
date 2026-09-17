import 'package:app/src/rust/api/book.dart';
import 'package:app/ui/cover_editor_page.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'rejecting a required whole-book download starts no download work',
    () async {
      final gateway = _FakeCustomCoverGateway(
        safeResult: const NeedsWholeBookDownload(),
      );
      final controller = CustomCoverOpenController(gateway);

      final decision = await controller.prepare();

      expect(decision, isA<NeedsWholeBookDownload>());
      expect(gateway.wholeBookCalls, 0);
    },
  );

  test(
    'a rejected whole-book decision can be prepared again without download work',
    () async {
      final gateway = _FakeCustomCoverGateway(
        safeResult: const NeedsWholeBookDownload(),
      );
      final controller = CustomCoverOpenController(gateway);

      await controller.prepare();
      final retried = await controller.prepare();

      expect(retried, isA<NeedsWholeBookDownload>());
      expect(gateway.wholeBookCalls, 0);
    },
  );

  test(
    'confirmation starts one explicit whole-book download after safe refusal',
    () async {
      final gateway = _FakeCustomCoverGateway(
        safeResult: const NeedsWholeBookDownload(),
      );
      final controller = CustomCoverOpenController(gateway);

      final decision = await controller.prepare();
      expect(decision, isA<NeedsWholeBookDownload>());
      final book = await controller.confirmWholeBookDownload();

      expect(book.title, 'downloaded');
      expect(gateway.safeCalls, 1);
      expect(gateway.wholeBookCalls, 1);
    },
  );

  test(
    'range-supported custom cover opens safely without confirmation or download',
    () async {
      final gateway = _FakeCustomCoverGateway(
        safeResult: CustomCoverReady(_safeBook),
      );
      final controller = CustomCoverOpenController(gateway);

      final decision = await controller.prepare();

      expect(decision, CustomCoverReady(_safeBook));
      expect(gateway.safeCalls, 1);
      expect(gateway.wholeBookCalls, 0);
    },
  );
}

final _safeBook = BookInfo(handle: BigInt.one, title: 'safe', pageCount: 1);

class _FakeCustomCoverGateway implements CustomCoverGateway {
  final CustomCoverOpenDecision safeResult;
  int safeCalls = 0;
  int wholeBookCalls = 0;

  _FakeCustomCoverGateway({required this.safeResult});

  @override
  Future<BookInfo?> openCached() async => null;

  @override
  Future<CustomCoverOpenDecision> openSafePartial() async {
    safeCalls++;
    return safeResult;
  }

  @override
  Future<BookInfo> openWholeBookDownload() async {
    wholeBookCalls++;
    return BookInfo(handle: BigInt.two, title: 'downloaded', pageCount: 1);
  }
}
