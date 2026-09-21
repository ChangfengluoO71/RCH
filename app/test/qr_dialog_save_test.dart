import 'package:app/ui/cloud115_qr_scan.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';

/// 115 扫码对话框的**构造契约**（2026-09-21 重写）。
///
/// **背景**：旧测试通过 `imageSaver:` 注入一个假的相册保存实现来断言
/// “保存二维码 → 导出 PNG / 失败可重试 / 窄屏可用”。该注入点**已不存在**
/// （对话框现在只接受 uid/time/sign/qrcode/app/onError）⇒ CI `flutter analyze` 3 处 error。
///
/// 为什么改写而不是删除：**构造契约本身仍值得锁定**（二维码内容与回调必须真的传进对话框）。
/// 至于保存路径（`_saveQrToGallery` 走平台通道）：要恢复那部分可测性，
/// 需要重新引入可注入的保存实现（见 `.trellis/spec/backend/remote-cover-update-contracts.md`
/// 中关于“可注入边界”的说明）。在那之前不对不存在的能力写测试。
void main() {
  Cloud115CookieQrScanDialog dialog({void Function(String)? onError}) =>
      Cloud115CookieQrScanDialog(
        uid: 'u1',
        time: PlatformInt64Util.from(0),
        sign: 's1',
        qrcode: 'https://115.com/scan/test',
        app: 'wechatmini',
        onError: onError,
      );

  test('二维码内容与回调被原样接线到对话框', () {
    var errors = 0;
    final widget = dialog(onError: (_) => errors++);

    expect(widget.uid, 'u1');
    expect(widget.sign, 's1');
    expect(widget.qrcode, 'https://115.com/scan/test');
    expect(widget.app, 'wechatmini');
    expect(widget.onError, isNotNull);
  });

  test('onError 是可选的（不传也能构造）', () {
    expect(dialog().onError, isNull);
  });
}
