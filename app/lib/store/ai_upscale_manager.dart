import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';

import '../repository/tag_repository.dart';
import '../src/rust/api/ai.dart';
import '../src/rust/api/book.dart';
import '../src/rust/api/db.dart';
import '../src/rust/api/source.dart';
import 'library_store.dart';
import 'models.dart';
import 'webdav_session.dart';

enum AiTaskStatus { queued, running, canceled, done }

/// 一条 AI 超分后台任务。
class AiTask {
  AiTask({
    required this.id,
    required this.bookKey,
    required this.sourceType,
    required this.sourceId,
    required this.path,
    required this.title,
    this.scale = 2,
    this.total = 0,
    this.done = 0,
    this.status = AiTaskStatus.queued,
    int? createdAt,
    int? updatedAt,
  })  : createdAt = createdAt ?? DateTime.now().millisecondsSinceEpoch,
        updatedAt = updatedAt ?? DateTime.now().millisecondsSinceEpoch;

  final String id;
  final String bookKey;
  final String sourceType;
  final String sourceId;
  final String path;
  final String title;
  final int scale;
  int total;
  int done;
  AiTaskStatus status;
  final int createdAt;
  int updatedAt;

  bool get isActive => status == AiTaskStatus.queued || status == AiTaskStatus.running;

  AiTaskDto toDto() => AiTaskDto(
        id: id,
        bookKey: bookKey,
        sourceType: sourceType,
        sourceId: sourceId,
        path: path,
        title: title,
        scale: scale,
        total: total,
        done: done,
        status: status.name,
        createdAt: createdAt,
        updatedAt: updatedAt,
      );

  factory AiTask.fromDto(AiTaskDto d) => AiTask(
        id: d.id,
        bookKey: d.bookKey,
        sourceType: d.sourceType,
        sourceId: d.sourceId,
        path: d.path,
        title: d.title,
        scale: d.scale.toInt(),
        total: d.total.toInt(),
        done: d.done.toInt(),
        status: AiTaskStatus.values.firstWhere(
          (s) => s.name == d.status,
          orElse: () => AiTaskStatus.queued,
        ),
        createdAt: d.createdAt.toInt(),
        updatedAt: d.updatedAt.toInt(),
      );
}

/// AI 整本超分后台任务管理器（队列 + 持久化 + 进度 + 完成提示）。
class AiUpscaleManager extends ChangeNotifier {
  AiUpscaleManager._();
  static final AiUpscaleManager instance = AiUpscaleManager._();

  /// 全局导航 key：完成提示对话框使用。
  static final GlobalKey<NavigatorState> navigatorKey = GlobalKey<NavigatorState>();

  // 块越小进度更新越频繁（10 页/块：33 页 → 4 次可见更新，模型加载开销可接受）。
  static const int _chunk = 10;

  final List<AiTask> _tasks = [];
  bool _workerRunning = false;
  String? _readingBookKey;
  String? _forceAiVersionBookKey;
  String? _lastCompletedTitle;
  String? _lastFailedMessage;
  Timer? _completedTimer;

  List<AiTask> get tasks => List.unmodifiable(_tasks);
  String? get readingBookKey => _readingBookKey;
  String? get forceAiVersionBookKey => _forceAiVersionBookKey;
  String? get lastCompletedTitle => _lastCompletedTitle;
  String? get lastFailedMessage => _lastFailedMessage;

  /// 启动时加载持久化任务并续跑。
  Future<void> init() async {
    try {
      final dtos = await dbLoadAllAiTasks();
      _tasks.clear();
      for (final dto in dtos) {
        final t = AiTask.fromDto(dto);
        if (t.isActive) {
          t.status = AiTaskStatus.queued; // 重启恢复
          _tasks.add(t);
        } else {
          // 清理上次残留的 done/canceled 行
          try {
            await dbDeleteAiTask(id: t.id);
          } catch (_) {}
        }
      }
    } catch (_) {}
    notifyListeners();
    _kickWorker();
  }

  /// 加入队列（同一本书已有进行中/排队任务时忽略）。
  Future<void> enqueue({
    required BookSource source,
    required String path,
    required String title,
    int scale = 2,
  }) async {
    final bookKey = '${source.type}|${source.id}|$path';
    if (_tasks.any((t) => t.bookKey == bookKey && t.isActive)) return;
    final t = AiTask(
      id: '${DateTime.now().microsecondsSinceEpoch}',
      bookKey: bookKey,
      sourceType: source.type,
      sourceId: source.id,
      path: path,
      title: title,
      scale: scale,
    );
    _tasks.add(t);
    await _persist(t);
    notifyListeners();
    _kickWorker();
  }

  /// 取消任务（进行中的 CLI 调用跑完当前块后停止；已完成页缓存保留）。
  Future<void> cancel(String id) async {
    final t = _byId(id);
    if (t == null || !t.isActive) return;
    final wasRunning = t.status == AiTaskStatus.running;
    t.status = AiTaskStatus.canceled;
    t.updatedAt = DateTime.now().millisecondsSinceEpoch;
    await _persist(t);
    if (!wasRunning) {
      // 尚未开始执行（排队中）→ 直接移除，避免残留 canceled 行
      await _deleteAndRemove(t);
    }
    notifyListeners();
  }

  /// ReaderPage 挂载时注册、卸载时注销当前阅读的书。
  void setReadingBook(String? bookKey) {
    if (_readingBookKey == bookKey) return;
    _readingBookKey = bookKey;
    notifyListeners();
  }

  /// 消费"强制加载超分版本"标记（阅读器确认后调用）。
  void consumeForceAiVersion() {
    _forceAiVersionBookKey = null;
    notifyListeners();
  }

  void _kickWorker() {
    if (_workerRunning) return;
    _workerRunning = true;
    unawaited(_worker());
  }

  Future<void> _worker() async {
    try {
      while (true) {
        final task = _nextQueued();
        if (task == null) break;
        try {
          await _runTask(task);
        } catch (e) {
          debugPrint('[AiUpscale] 任务异常: $e');
          await _finishFailed(task, '$e');
        }
      }
    } finally {
      _workerRunning = false;
    }
  }

  Future<void> _runTask(AiTask task) async {
    task.status = AiTaskStatus.running;
    task.updatedAt = DateTime.now().millisecondsSinceEpoch;
    await _persist(task);
    notifyListeners();

    final store = LibraryStore.instance;
    BookSource? source;
    for (final s in store.sources) {
      if (s.id == task.sourceId) {
        source = s;
        break;
      }
    }
    if (source == null) {
      await _finishFailed(task, '书源不存在（可能已删除）');
      return;
    }

    try {
      final bk = source.isWebDav
          ? await openWebdavBook(session: await webdavSessionFor(source), path: task.path)
              .timeout(const Duration(seconds: 60))
          : await openLocalBook(path: task.path).timeout(const Duration(seconds: 60));
      task.total = bk.pageCount;
      task.updatedAt = DateTime.now().millisecondsSinceEpoch;
      await _persist(task);
      notifyListeners();

      for (var start = 0; start < task.total; start += _chunk) {
        if (task.status == AiTaskStatus.canceled) break;
        final end = start + _chunk < task.total ? start + _chunk : task.total;
        try {
          final pages = <Uint8List>[];
          for (var i = start; i < end; i++) {
            pages.add(await bookPage(handle: bk.handle, index: i)
                .timeout(const Duration(seconds: 60)));
          }
          final results = await superResolveBatch(pages: pages, scale: task.scale)
              .timeout(const Duration(minutes: 5));
          for (final r in results) {
            if (r.isNotEmpty) task.done++;
          }
        } catch (_) {
          // 整块失败：跳过，任务继续
        }
        task.updatedAt = DateTime.now().millisecondsSinceEpoch;
        await _persist(task);
        notifyListeners();
        await _logProgress('进度 ${task.status.name} ${task.done}/${task.total} ${task.title}');
        // 让出一帧：缓存全命中时任务可能在几十毫秒内跑完，
        // 不等待会导致悬浮窗只看到 0 → 完成 的跳变。
        await Future<void>.delayed(const Duration(milliseconds: 32));
      }
      try {
        closeBook(handle: bk.handle);
      } catch (_) {}

      if (task.status == AiTaskStatus.canceled) {
        await _deleteAndRemove(task);
        notifyListeners();
        return;
      }

      task.status = AiTaskStatus.done;
      task.updatedAt = DateTime.now().millisecondsSinceEpoch;
      await _persist(task);
      notifyListeners();

      if (task.done > 0) {
        TagRepository.instance.link(task.bookKey, 'AI超分');
        await store.saveToDisk();
      }

      await _notifyCompletion(task);
      await _deleteAndRemove(task);
      notifyListeners();
    } catch (e) {
      await _finishFailed(task, '$e');
    }
  }

  Future<void> _finishFailed(AiTask task, String reason) async {
    task.status = AiTaskStatus.canceled;
    task.updatedAt = DateTime.now().millisecondsSinceEpoch;
    await _persist(task);
    await _deleteAndRemove(task);
    debugPrint('[AiUpscale] 任务失败: $reason');
    _showFailedNotice('《${task.title}》超分失败: $reason');
    notifyListeners();
  }

  void _showFailedNotice(String message) {
    _lastFailedMessage = message;
    _completedTimer?.cancel();
    _completedTimer = Timer(const Duration(seconds: 5), () {
      _lastFailedMessage = null;
      notifyListeners();
    });
    notifyListeners();
  }

  /// 完成提示：正在阅读该书 → 弹切换对话框；否则悬浮窗显示"完成"提示条。
  Future<void> _notifyCompletion(AiTask task) async {
    final ctx = navigatorKey.currentContext;
    if (ctx == null || _readingBookKey != task.bookKey) {
      _showCompletedNotice(task.title);
      return;
    }
    final go = await showDialog<bool>(
      context: ctx,
      builder: (c) => AlertDialog(
        title: const Text('AI 超分完成'),
        content: Text('《${task.title}》已超分完毕，是否全部加载为超分版本？'),
        actions: [
          TextButton(onPressed: () => Navigator.of(c).pop(false), child: const Text('暂不')),
          FilledButton(onPressed: () => Navigator.of(c).pop(true), child: const Text('加载超分版本')),
        ],
      ),
    );
    if (go == true) {
      _forceAiVersionBookKey = task.bookKey;
      notifyListeners();
    }
  }

  void _showCompletedNotice(String title) {
    _lastCompletedTitle = title;
    _completedTimer?.cancel();
    _completedTimer = Timer(const Duration(seconds: 3), () {
      _lastCompletedTitle = null;
      notifyListeners();
    });
    notifyListeners();
  }

  AiTask? _byId(String id) {
    for (final t in _tasks) {
      if (t.id == id) return t;
    }
    return null;
  }

  AiTask? _nextQueued() {
    for (final t in _tasks) {
      if (t.status == AiTaskStatus.queued) return t;
    }
    return null;
  }

  Future<void> _persist(AiTask t) async {
    try {
      await dbUpsertAiTask(task: t.toDto());
    } catch (_) {}
  }

  /// 追加进度日志（定位"不显示中间进度"用）：每块变化都会落盘。
  static Future<void> _logProgress(String msg) async {
    try {
      final dir = await getApplicationSupportDirectory();
      final f = File('${dir.path}${Platform.pathSeparator}ai_progress.log');
      await f.writeAsString('${DateTime.now().toIso8601String()} $msg\n',
          mode: FileMode.append);
    } catch (_) {}
  }

  Future<void> _deleteAndRemove(AiTask t) async {
    try {
      await dbDeleteAiTask(id: t.id);
    } catch (_) {}
    _tasks.remove(t);
  }
}
