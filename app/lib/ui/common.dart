import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/services.dart';

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
