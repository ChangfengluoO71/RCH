import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:app/store/library_store.dart';

/// 窄屏断点：逻辑宽度 < 600dp 视为紧凑（手机）布局。
const double kCompactBreakpoint = 600;

class LayoutProfile {
  const LayoutProfile({
    required this.tablet,
    required this.compact,
    required this.lockPortrait,
  });

  final bool tablet;
  final bool compact;
  final bool lockPortrait;
}

/// The chosen experience is independent of the current window width. Android
/// display size (rather than split-screen window size) identifies auto tablets.
LayoutProfile resolveLayoutProfile(BuildContext context) {
  final window = MediaQuery.sizeOf(context);
  final selection = LibraryStore.instance.settings.tabletLayout;
  final display = View.maybeOf(context)?.display;
  final displaySize = display == null || display.devicePixelRatio <= 0
      ? window
      : display.size / display.devicePixelRatio;
  final shortSide = displaySize.shortestSide > 0
      ? displaySize.shortestSide
      : window.shortestSide;
  final androidTablet = isAndroidPlatform && shortSide >= kCompactBreakpoint;
  final tablet =
      selection == 'tablet' || (selection == 'auto' && androidTablet);
  final compact = window.width < kCompactBreakpoint || selection == 'mobile';
  final lockPortrait =
      !tablet && (selection == 'auto' && isAndroidPlatform ? true : compact);
  return LayoutProfile(
    tablet: tablet,
    compact: compact,
    lockPortrait: lockPortrait,
  );
}

bool isCompact(BuildContext context) => resolveLayoutProfile(context).compact;

/// Keeps the home policy current while a reader temporarily permits rotation.
class AppOrientationCoordinator {
  bool _homePortrait = true;
  bool? _appliedPortrait;
  Object? _readerOwner;

  bool get readerActive => _readerOwner != null;

  void updateHome(LayoutProfile profile) {
    if (!isAndroidPlatform) return;
    _homePortrait = profile.lockPortrait;
    if (!readerActive) _apply(_homePortrait);
  }

  void enterReader(Object owner) {
    if (!isAndroidPlatform) return;
    _readerOwner = owner;
    _apply(false);
  }

  void leaveReader(Object owner) {
    if (!isAndroidPlatform || !identical(_readerOwner, owner)) return;
    _readerOwner = null;
    _apply(_homePortrait);
  }

  void _apply(bool portrait) {
    if (_appliedPortrait == portrait) return;
    _appliedPortrait = portrait;
    SystemChrome.setPreferredOrientations(
      portrait ? [DeviceOrientation.portraitUp] : DeviceOrientation.values,
    );
  }
}

final appOrientationCoordinator = AppOrientationCoordinator();

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
