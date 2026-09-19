import 'dart:io';

import 'package:app/src/rust/api/cache.dart';
import 'package:flutter/foundation.dart';

/// RG-B：**扫描诊断日志**（`<cache root>/scan_diag.log`）。
///
/// 为什么需要：扫描启动被拒、残留 running 被恢复等**已被业务处理**的失败不会进入
/// `errors.log`（那里只记录未捕获异常），导致线上"查不到日志"。这里给出一个最小的、
/// 本地可查的诊断通道：只写**安全枚举码**与源标识，不写凭据、不写私有 URL。
Future<void> appendScanDiag(String line) async {
  try {
    final root = await cacheRootPath();
    final file = File('$root${Platform.pathSeparator}scan_diag.log');
    final sink = file.openWrite(mode: FileMode.append);
    sink.writeln('${DateTime.now().toIso8601String()} $line');
    await sink.close();
  } catch (error) {
    // 诊断日志失败绝不影响业务。
    debugPrint('[scan_diag] append failed: $error');
  }
}
