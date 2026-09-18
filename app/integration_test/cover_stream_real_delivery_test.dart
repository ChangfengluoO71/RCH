// RG-B / B-1：**真实 FRB `StreamSink` 跨桥 delivery** 的集成证据。
//
// 目标（**窄**，不重造 A-4 矩阵）：证明一条真实跨桥链 ——
//   真实 app 启动 → 无注入 fake stream（`debugCoverRevisionStream == null`，仅辅助证据）
//   → production WebDAV 源 → 扫描已 terminal → Rust cover durable transition
//   → Rust `StreamSink` → Dart coordinator → **只读** durable reread
//   → UI 在**测试不驱动任何读取**的条件下自动从非 ready 变为 ready。
//
// 运行（真实 Windows 桌面目标 + 真实 FRB 原生库）：
//   flutter test integration_test/cover_stream_real_delivery_test.dart -d windows
//
// 证据口径：
// * 本用例的 authority 是"**Rust 状态变化后，在没有其他 wake source 的条件下 UI 自动跟随变化**"；
//   `debugCoverRevisionStream == null` 只证明没有注入 fake stream，**不能单独证明**事件真的到了 Dart。
// * dedup / catch-up / request-once / no-polling 等消费语义由 A-4 的 Dart focused 测试覆盖
//   （fresh 13/13），本用例**不**重建该矩阵。
// * 本文件属 **Validation Harness**，不改变 RC product identity（RC Product Commit = b8b64de）。

import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:app/main.dart' as app;
import 'package:app/src/rust/api/db.dart';
import 'package:app/src/rust/api/remote_cover.dart' as rust_cover;
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:app/store/webdav_session.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

/// 1×1 透明 PNG（canonical，67 字节；无需在测试内计算 CRC）。
final Uint8List _tinyPng = Uint8List.fromList(<int>[
  0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, //
  0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
  0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
  0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
  0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41,
  0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
  0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
  0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
  0x42, 0x60, 0x82,
]);

/// 极简 WebDAV 源：`/Series/{001,002}.png` ⇒ 生产扫描判定为 `ImageFolder`（漫画文件夹）。
///
/// 只实现生产路径真正需要的四件事：PROPFIND（含 `getcontentlength`，`file_size()` 依赖）、
/// HEAD（RTT 探测）、Range GET（206，且对 `bytes=0-0` **忠实回显**——分类器要求
/// `start==0 && end==0`，否则判 MalformedResponse）、普通 GET（200 全量）。
class _WebDavFixture {
  _WebDavFixture._(this._server) {
    _server.listen(_handle);
  }

  final HttpServer _server;
  final List<String> requestLog = <String>[];

  static Future<_WebDavFixture> start() async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    return _WebDavFixture._(server);
  }

  int get port => _server.port;
  String get baseUrl => 'http://127.0.0.1:$port';
  Uri get seriesUrl => Uri.parse('$baseUrl/Series/001.png');

  Future<void> close() => _server.close(force: true);

  /// 形如 `001.png` 的条目（供 multistatus 与 GET 共用）。
  static const _files = <String>['001.png', '002.png'];

  Future<void> _handle(HttpRequest request) async {
    final path = request.uri.path;
    final range = request.headers.value(HttpHeaders.rangeHeader);
    requestLog.add('${request.method} $path range=${range ?? '-'}');

    try {
      if (request.method == 'PROPFIND') {
        await _propfind(request, path);
      } else if (request.method == 'HEAD') {
        await _head(request, path);
      } else if (request.method == 'GET') {
        await _get(request, path, range);
      } else {
        request.response.statusCode = HttpStatus.methodNotAllowed;
        await request.response.close();
      }
    } catch (_) {
      // 夹具自身的异常不应污染被测断言。
      try {
        await request.response.close();
      } catch (_) {}
    }
  }

  Future<void> _propfind(HttpRequest request, String path) async {
    final normalized = path.endsWith('/') ? path : '$path/';
    final buffer = StringBuffer('<?xml version="1.0" encoding="utf-8"?>')
      ..write('<D:multistatus xmlns:D="DAV:">');
    if (normalized == '/' || normalized.isEmpty) {
      // 根：只包含 Series 目录（作为 collection）。
      buffer
        ..write('<D:response><D:href>/Series/</D:href><D:propstat><D:prop>')
        ..write('<D:displayname>Series</D:displayname>')
        ..write('<D:getcontentlength>0</D:getcontentlength>')
        ..write('<D:resourcetype><D:collection/></D:resourcetype>')
        ..write('</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>');
    } else {
      // /Series/：两个图片文件。
      for (final name in _files) {
        buffer
          ..write('<D:response><D:href>/Series/$name</D:href><D:propstat><D:prop>')
          ..write('<D:displayname>$name</D:displayname>')
          ..write('<D:getcontentlength>${_tinyPng.length}</D:getcontentlength>')
          ..write('<D:resourcetype/>')
          ..write('</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>');
      }
    }
    buffer.write('</D:multistatus>');
    final body = utf8.encode(buffer.toString());
    final response = request.response
      ..statusCode = 207
      ..headers.contentType = ContentType.parse('application/xml; charset=utf-8')
      ..headers.contentLength = body.length;
    response.add(body);
    await response.close();
  }

  Future<void> _head(HttpRequest request, String path) async {
    final size = path.endsWith('.png') ? _tinyPng.length : 0;
    final response = request.response
      ..statusCode = HttpStatus.ok
      ..headers.contentLength = size
      ..headers.set('Accept-Ranges', 'bytes');
    await response.close();
  }

  Future<void> _get(HttpRequest request, String path, String? range) async {
    if (!path.endsWith('.png')) {
      // 真实 WebDAV 对 collection 的普通 GET 返回 200（HTML/XML 列表）。
      // 生产 connect 阶段会对 browsing root 做 range probe（GET），因此这里不能 404。
      final body = utf8.encode('ok');
      final response = request.response
        ..statusCode = HttpStatus.ok
        ..headers.contentType = ContentType.text
        ..headers.contentLength = body.length;
      response.add(body);
      await response.close();
      return;
    }
    final total = _tinyPng.length;
    if (range != null && range.startsWith('bytes=')) {
      final spec = range.substring('bytes='.length).split(',').first.trim();
      final parts = spec.split('-');
      final start = int.tryParse(parts.first) ?? 0;
      final end = parts.length > 1 && parts[1].isNotEmpty
          ? int.parse(parts[1])
          : total - 1;
      final slice = _tinyPng.sublist(start, end + 1);
      final response = request.response
        ..statusCode = HttpStatus.partialContent
        ..headers.contentLength = slice.length
        ..headers.set(
          HttpHeaders.contentRangeHeader,
          'bytes $start-$end/$total',
        )
        ..headers.set('Accept-Ranges', 'bytes');
      response.add(slice);
      await response.close();
      return;
    }
    final response = request.response
      ..statusCode = HttpStatus.ok
      ..headers.contentLength = total
      ..headers.set('Accept-Ranges', 'bytes');
    response.add(_tinyPng);
    await response.close();
  }
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets(
    'B-1: real Rust StreamSink wake reaches the real Dart coordinator and flips the card',
    (WidgetTester tester) async {
      final fixture = await _WebDavFixture.start();
      addTearDown(fixture.close);

      // **真实启动路径**：执行生产 `main()` —— 它内含 RustLib.init()、数据迁移、
      // restoreStatuses()、**startCoverRevisionWatch()（唯一真实 StreamSink 订阅）**
      // 与 runApp(const RchApp())。
      //
      // 关键：此前只 `pumpWidget(const RchApp())` 并不会执行 main()，因此从未建立订阅，
      // commit 之后的 wake 无人消费（durable ready 而 UI 冻结）。这正是本 harness
      // 之前的缺口，而非生产缺陷。
      await app.main();
      await tester.pumpAndSettle(const Duration(milliseconds: 800));

      // 辅助证据：没有注入 fake stream ⇒ 走的是真实 subscribeCoverRevisions()。
      expect(
        RemoteScanCoordinator.instance.debugCoverRevisionStream,
        isNull,
        reason: 'no injected stream may be used for this evidence',
      );

      // ---- production 源 + session ----
      final source = BookSource(
        id: 'b1-real-delivery',
        type: 'webdav',
        name: 'B1 Real Delivery',
        url: fixture.baseUrl,
        username: 'u',
        password: 'p',
      );
      await LibraryStore.instance.addSource(source);
      // ADR-020：源身份（fingerprint）由 **Rust** 统一计算并落库。生产加源流程会调用
      // `dbUpsertSource`；随后 `remoteDirectoryView` / 扫描才能解析 source identity。
      // 这是**测试装配**步骤（注册源），不是驱动封面读取。
      await dbUpsertSource(
        source: BookSourceDto(
          id: source.id,
          type: source.type,
          name: source.name,
          path: source.path,
          url: source.url,
          username: source.username,
          password: source.password,
          note: source.note,
          capabilityLabel: source.capabilityLabel,
          remoteOnly: source.remoteOnly,
        ),
      );
      final BigInt session;
      try {
        session = await webdavSessionFor(source);
      } catch (error) {
        // ignore: avoid_print
        print('RG_B1_CONNECT_FAILED=$error;requests=${fixture.requestLog}');
        rethrow;
      }

      // ---- production 扫描，等到 terminal（不是 cover polling）----
      await RemoteScanCoordinator.instance.rescanFull(source, session);
      final deadline = DateTime.now().add(const Duration(seconds: 60));
      var terminal = false;
      await tester.runAsync(() async {
        while (DateTime.now().isBefore(deadline)) {
          final status = RemoteScanCoordinator.instance.statusFor(source.id).value;
          if (status != null &&
              (status.status == 'complete' ||
                  status.status == 'degraded' ||
                  status.status == 'failed')) {
            terminal = true;
            break;
          }
          await Future<void>.delayed(const Duration(milliseconds: 200));
        }
      });
      await tester.pump();
      final scanStatus = RemoteScanCoordinator.instance.statusFor(source.id).value;
      // ignore: avoid_print
      print(
        'RG_B1_SCAN=status=${scanStatus?.status};error=${scanStatus?.errorCode};'
        'discovered=${scanStatus?.discoveredBooks};dirs=${scanStatus?.directoriesChecked};'
        'listing=${scanStatus?.listingPhase};requests=${fixture.requestLog}',
      );
      expect(terminal, isTrue, reason: 'scan must reach a terminal state');
      expect(
        scanStatus?.status,
        'complete',
        reason: 'the scan must SUCCEED for the cover chain to exist',
      );

      // ---- 取生产 listing 的目录资产身份（ImageFolder ⇒ 用目录 assetId）----
      final rust_cover.RemoteDirectoryViewDto view;
      try {
        view = await rust_cover.remoteDirectoryView(
          sourceId: source.id,
          logicalPath: '/',
          offset: 0,
          limit: 50,
        );
      } catch (error) {
        // ignore: avoid_print
        print('RG_B1_DIRECTORY_VIEW_FAILED=$error;requests=${fixture.requestLog}');
        rethrow;
      }
      final series = view.entries.firstWhere(
        (e) => e.name == 'Series',
        orElse: () => throw StateError('listing must expose the Series folder'),
      );
      expect(series.assetKind, 'ImageFolder');

      // ---- 挂载生产卡片（此后测试**不再**驱动任何读取）----
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Center(
              child: SizedBox(
                width: 64,
                height: 64,
                child: ComicCover(
                  source: source,
                  path: series.logicalPath,
                  remoteAssetId: series.assetId,
                  remoteSession: session,
                  preferUnifiedRemote: true,
                ),
              ),
            ),
          ),
        ),
      );
      await tester.runAsync(() async {
        for (var i = 0; i < 10; i++) {
          await Future<void>.delayed(const Duration(milliseconds: 50));
        }
      });
      await tester.pump();

      // 起始必须**非 ready**（否则链无从被证明）。
      expect(
        find.byType(RawImage),
        findsNothing,
        reason: 'the card must start without cover material',
      );

      // ---- 关键观察窗：测试不调用 readState / 不 refresh / 不重开 / 不重 request ----
      // 仅等待真实 Rust worker 完成 → commit → StreamSink → coordinator → 卡片自行更新。
      var flipped = false;
      final flipDeadline = DateTime.now().add(const Duration(seconds: 90));
      await tester.runAsync(() async {
        while (DateTime.now().isBefore(flipDeadline)) {
          await Future<void>.delayed(const Duration(milliseconds: 200));
          await tester.pump();
          if (find.byType(RawImage).evaluate().isNotEmpty) {
            flipped = true;
            break;
          }
        }
      });
      await tester.pump();

      // 观察窗**已结束**后才做诊断性读取（用于判断失败层级：durable 未 ready vs UI 未跟随）。
      final durable = await rust_cover.remoteCoverState(
        sourceId: source.id,
        assetId: series.assetId,
      );
      final aggregate = RemoteScanCoordinator.instance.statusFor(source.id).value;
      // ignore: avoid_print
      print(
        'RG_B1_AFTER_WAIT=flipped=$flipped;durable_state=${durable?.state};'
        'durable_ready=${durable?.ready};durable_error=${durable?.errorCode};'
        'ready_books=${aggregate?.readyBooks};available_books=${aggregate?.availableBooks};'
        'requests=${fixture.requestLog}',
      );

      expect(
        flipped,
        isTrue,
        reason:
            'the cover card must flip to ready purely from the real FRB stream wake '
            '(no test-driven readState / refresh / reopen / re-request); '
            'durable_state=${durable?.state} durable_ready=${durable?.ready} '
            'durable_error=${durable?.errorCode}',
      );

      // 佐证：durable 状态确已 ready。
      expect(durable, isNotNull, reason: 'durable cover state must exist');
      expect(durable!.ready, isTrue);
      expect(durable.state, 'ready');

      // 可机读证据行（product commit 固定；harness commit 由报告记录）。
      // ignore: avoid_print
      print(
        'RG_B1_FRB_DELIVERY=PASS;candidate=0.5.8+100508;product_commit=b8b64de;'
        'source=${source.id};asset=${series.assetId};'
        'requests=${fixture.requestLog.length};injected_stream=false',
      );
    },
  );
}
