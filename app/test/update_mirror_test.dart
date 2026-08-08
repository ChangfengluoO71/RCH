import 'dart:convert';

import 'package:app/store/models.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/update_manager.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('UpdateManager.buildDownloadUrl', () {
    const official =
        'https://github.com/ChangfengluoO71/RCH/releases/download/v0.4.2/'
        'RCH-0.4.2-windows-x64.exe';

    test('镜像为空时返回官方直链', () {
      expect(UpdateManager.buildDownloadUrl(official, ''), official);
      expect(UpdateManager.buildDownloadUrl(official, '   '), official);
    });

    test('前缀代理拼接：前缀带/或不带/结果一致', () {
      const expected =
          'https://ghproxy.net/https://github.com/ChangfengluoO71/RCH/'
          'releases/download/v0.4.2/RCH-0.4.2-windows-x64.exe';
      expect(UpdateManager.buildDownloadUrl(official, 'https://ghproxy.net/'),
          expected);
      expect(UpdateManager.buildDownloadUrl(official, 'https://ghproxy.net'),
          expected);
    });

    test('downloadCandidates：当前选择优先、去重、官方直连保留', () {
      final mirrors = UpdateManager.mirrorPresets;
      // 默认官方直连：第一位是 ''
      var c = UpdateManager.downloadCandidates('', mirrors);
      expect(c.first, '');
      expect(c.length, mirrors.length);
      // 选中镜像后它排第一，其余去重
      c = UpdateManager.downloadCandidates('https://ghfast.top/', mirrors);
      expect(c.first, 'https://ghfast.top/');
      expect(c.where((u) => u == 'https://ghfast.top/').length, 1);
      // 自定义镜像不在预设中也会排第一
      c = UpdateManager.downloadCandidates(
          'https://my.example.com/', mirrors);
      expect(c.first, 'https://my.example.com/');
    });
  });

  test('AppSettings updateMirror 序列化往返', () {
    final s = AppSettings(
      updateMirror: 'https://ghfast.top/',
      updateMirrorList: jsonEncode([
        {'name': 'ghfast.top', 'url': 'https://ghfast.top/'},
      ]),
      updateMirrorFetchedAt: 1234567890,
    );
    final back = AppSettings.fromJson(s.toJson());
    expect(back.updateMirror, 'https://ghfast.top/');
    expect(back.updateMirrorList, contains('ghfast.top'));
    expect(back.updateMirrorFetchedAt, 1234567890);

    final d = AppSettings();
    expect(AppSettings.fromJson(d.toJson()).updateMirror, '');
    expect(AppSettings.fromJson(d.toJson()).updateMirrorList, '[]');
    expect(AppSettings.fromJson(d.toJson()).updateMirrorFetchedAt, 0);
  });

  test('effectiveMirrors：远端列表在前、内置预设兜底、按 URL 去重', () {
    LibraryStore.instance.settings.updateMirrorList = jsonEncode([
      {'name': 'ghfast.top', 'url': 'https://ghfast.top/'},
      {'name': 'ghproxy.net', 'url': 'https://ghproxy.net/'},
    ]);
    final mirrors = UpdateManager.instance.effectiveMirrors;
    final urls = mirrors.map((m) => m.value).toList();
    // 远端镜像在前
    expect(urls.indexOf('https://ghfast.top/') <
        urls.indexOf('https://gh-proxy.com/'), true);
    // 去重
    expect(urls.where((u) => u == 'https://ghfast.top/').length, 1);
    // 内置预设兜底仍在
    expect(urls, contains('https://gh-proxy.com/'));
    expect(urls, contains('https://mirror.ghproxy.com/'));
  });
}
