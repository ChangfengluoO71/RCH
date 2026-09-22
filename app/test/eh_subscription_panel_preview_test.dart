// EH 订阅面板的视觉核对（一次性渲染为 PNG，供人眼检查布局与配色）。
//
// 不依赖真机/FFI：通过 `EhSubscriptionStore.forTest` 注入预置状态。
// 产物写到 `build/eh_panel_preview.png`（由 `--dart-define` 指定输出目录亦可）。
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:app/store/eh_subscription_store.dart';
import 'package:app/ui/eh_subscription_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show ByteData, FontLoader;
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

/// 一份贴近真实结果的预置状态（数值取自 2026-09-22 实测）。
EhSubscriptionStore _store() => EhSubscriptionStore.forTest(
  rules: {
    'search': r'language:chinese$ uncensored',
    'min_rating': 4.0,
    'title_markers': 'Digital|DL版|DL',
    'exclude_markers': ['AI Generated'],
    'age_tiers': [
      {'min_age_years': 5.0, 'min_downloads': 800},
      {'min_age_years': 2.0, 'min_downloads': 500},
      {'min_age_years': 0.5, 'min_downloads': 300},
      {'min_age_years': 0.083, 'min_downloads': 100},
      {'min_age_years': 0.0, 'min_downloads': 0},
    ],
    'out_dir': r'D:\EhTorrents',
    'pages': 2,
    'host': 'e-hentai.org',
    'request_interval_secs': 2.5,
    'now_offset_days': 0,
  },
  manifest: [
    EhSavedItem(
      infohash: 'ea29160bfbebedfaa8c818310548ead6432c49ce',
      titleJpn: '[赤月屋 (赤月みゅうと)] 僕にしか触れないサキュバス三姉妹に搾られる話4〜長女レミィ編(前編)〜 [中国翻訳] [無修正] [DL版]',
      rating: 4.8,
      downloads: 926,
      requiredDl: 0,
      postedUtc: '2026-09-20 23:07 UTC',
      ageYears: 0.0,
      category: 'Doujinshi',
      filecount: 87,
      filesize: 135522052,
      tags: const ['language:chinese', 'other:uncensored'],
      file: 'ea29160bfbebedfaa8c818310548ead6432c49ce-....torrent',
      bytes: 10619,
    ),
    EhSavedItem(
      infohash: '2b34b49b065e0c7e5441b3cd4db9262aacfd2b30',
      titleJpn: '[陸の孤島亭 (しゃよー)] 田舎にはこれくらいしか娯楽がない5 [中国翻訳] [無修正] [DL版]',
      rating: 4.78,
      downloads: 875,
      requiredDl: 800,
      postedUtc: '2026-09-20 23:03 UTC',
      ageYears: 0.0,
      category: 'Doujinshi',
      filecount: 59,
      filesize: 87241523,
      tags: const ['language:chinese', 'other:uncensored', 'female:big breasts'],
      file: '2b34b49b065e0c7e5441b3cd4db9262aacfd2b30-....torrent',
      bytes: 6927,
    ),
  ],
  progress: const EhProgress(
    running: true,
    stage: 'probing',
    message: '查询下载数 12/16',
    page: 2,
    pages: 2,
    candidates: 50,
    checked: 12,
    saved: 13,
    noRating: 16,
    noMarker: 14,
    noTorrent: 2,
    noDownloads: 5,
    unmapped: 8,
  ),
);

void main() {
  // 测试环境默认用占位字体（Ahem），所有文字会渲染成方块，无法做视觉核对。
  // 这里加载系统中文字体（微软雅黑/等线），让预览图能真实反映排版与文案。
  setUpAll(() async {
    for (final entry in {
      'Microsoft YaHei': [r'C:\Windows\Fonts\msyh.ttc', r'C:\Windows\Fonts\msyhbd.ttc'],
      'DengXian': [r'C:\Windows\Fonts\Deng.ttf'],
    }.entries) {
      for (final path in entry.value) {
        final file = File(path);
        if (!file.existsSync()) continue;
        final loader = FontLoader(entry.key)
          ..addFont(Future.value(ByteData.sublistView(file.readAsBytesSync())));
        await loader.load();
      }
    }
  });

  testWidgets('渲染 EH 订阅面板预览图', (tester) async {
    // 用"运行中"状态渲染，但随后把 busy 置 false 再出图不现实；
    // 折中：进度区用固定帧渲染（无限动画只影响 pumpAndSettle，不影响出图）。
    tester.view.physicalSize = const Size(1280, 2400);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final key = GlobalKey();
    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(
          useMaterial3: true,
          colorSchemeSeed: const Color(0xFF6750A4),
          brightness: Brightness.light,
          // 与上一步加载的字体族对齐（否则仍会退回占位字体）
          fontFamily: 'Microsoft YaHei',
        ),
        home: Scaffold(
          body: RepaintBoundary(
            key: key,
            child: SingleChildScrollView(
              padding: const EdgeInsets.all(24),
              child: EhSubscriptionPanel(storeOverride: _store()),
            ),
          ),
        ),
      ),
    );
    // 注意：预置状态含 ProgressIndicator（无限动画），不能用 pumpAndSettle
    //（它会等所有动画结束 → 超时）。固定帧推进即可。
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    // 出图必须在真实时间轴上跑：测试默认走 fake-async，`toImage` 的 Future
    // 会在 fake 时钟里永远不完成（表现为测试 hang）。`runAsync` 是标准解法。
    late Uint8List png;
    await tester.runAsync(() async {
      final boundary = key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
      final image = await boundary.toImage(pixelRatio: 1.0);
      final data = await image.toByteData(format: ui.ImageByteFormat.png);
      expect(data, isNotNull, reason: '面板应能渲染出图像');
      png = data!.buffer.asUint8List();
      image.dispose();
    });

    final outDir = Directory('build');
    if (!outDir.existsSync()) outDir.createSync(recursive: true);
    final file = File('build/eh_panel_preview.png');
    file.writeAsBytesSync(png);

    // 兜底断言：确认面板确实渲染了关键控件（防止"截了个空白图"）。
    // 注意主按钮文案随状态变化（运行中为「扫描中…」，空闲为「开始扫描」）。
    expect(find.text('EH 订阅'), findsOneWidget);
    final primary = tester.widget<FilledButton>(find.byType(FilledButton).first);
    expect(find.text(primary.onPressed == null ? '扫描中…' : '开始扫描'), findsOneWidget);
    expect(find.text('搜索语法'), findsOneWidget);
    expect(find.text('已保存种子'), findsOneWidget);
    expect(find.text('分时间段下载数要求'), findsOneWidget);

    stdout.writeln('PREVIEW_WRITTEN ${file.absolute.path} bytes=${file.lengthSync()}');
    expect(jsonEncode({'ok': true}), contains('ok'));
  }, timeout: const Timeout(Duration(seconds: 90)));
}
