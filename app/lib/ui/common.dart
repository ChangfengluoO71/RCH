import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:app/store/library_store.dart';

/// 窄屏断点：逻辑宽度 < 600dp 视为紧凑（手机）布局。
const double kCompactBreakpoint = 600;

/// 当前是否使用紧凑（手机）布局：
/// - 宽度 < 600dp 恒为紧凑（手机）；
/// - 宽度 ≥ 600dp（平板/大屏）时遵循设置里的"平板布局"（auto/desktop = 桌面侧栏，mobile = 手机底部导航）。
bool isCompact(BuildContext context) {
  final w = MediaQuery.sizeOf(context).width;
  if (w < kCompactBreakpoint) return true;
  return LibraryStore.instance.settings.tabletLayout == 'mobile';
}

/// 对话框内容最大宽度：宽屏上限 420dp，窄屏自适应为屏宽减 48dp 边距。
double dialogMaxWidth(BuildContext context) {
  final screen = MediaQuery.sizeOf(context).width;
  return (screen - 48).clamp(0.0, 420.0);
}

/// 当前是否为 Android 平台（AI 超分等 Windows 专属能力在 Android 隐藏）。
bool get isAndroidPlatform =>
    !kIsWeb && defaultTargetPlatform == TargetPlatform.android;

/// 把 Rust 解码的 RGBA 像素转成 Flutter 可显示的 ui.Image(封面缩略图用)。
Future<ui.Image> rgbaToImage(Uint8List rgba, int w, int h) {
  final c = Completer<ui.Image>();
  ui.decodeImageFromPixels(rgba, w, h, ui.PixelFormat.rgba8888, c.complete);
  return c.future;
}

/// 把字节数格式化为易读大小(MB / GB)。
String fmtSize(BigInt bytes) {
  final d = bytes.toDouble();
  final mb = d / 1048576;
  return mb >= 1024
      ? '${(mb / 1024).toStringAsFixed(2)} GB'
      : '${mb.toStringAsFixed(1)} MB';
}

/// fmtSize 重载：接受 num 类型（CacheSize 生成的是 num）。
String fmtNum(num bytes) {
  final mb = bytes / 1048576;
  return mb >= 1024
      ? '${(mb / 1024).toStringAsFixed(2)} GB'
      : '${mb.toStringAsFixed(1)} MB';
}
