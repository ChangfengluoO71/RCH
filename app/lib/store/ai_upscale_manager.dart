import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';

import '../repository/tag_repository.dart';
import '../src/rust/api/ai.dart';
import '../src/rust/api/book.dart';
import '../src/rust/api/db.dart';
import '../src/rust/api/source.dart';
import 'baidu_session.dart';
import 'cloud115_session.dart';
import 'library_store.dart';
import 'models.dart';
import 'quark_session.dart';
import 'sftp_session.dart';
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
    this.sortOrder = 0,
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
  /// 排队顺序（进行中固定为 0，排队任务从 1 递增；拖拽后持久化）。
  int sortOrder;
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
        sortOrder: sortOrder,
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
        sortOrder: d.sortOrder.toInt(),
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

  final List<AiTask> _tasks = [];
  bool _workerRunning = false;
  String? _readingBookKey;
  String? _forceAiVersionBookKey;
  String? _lastCompletedTitle;
  String? _lastFailedMessage;
  Timer? _completedTimer;

  List<AiTask> get tasks => List.unmodifiable(_tasks);
  /// 展示用活动任务：进行中固定在前，排队任务按 sortOrder 升序。
  List<AiTask> get activeTasks {
    final running = _tasks.where((t) => t.status == AiTaskStatus.running).toList();
    final queued = _tasks.where((t) => t.status == AiTaskStatus.queued).toList()
      ..sort((a, b) => a.sortOrder.compareTo(b.sortOrder));
    return [...running, ...queued];
  }
  String? get readingBookKey => _readingBookKey;
  String? get forceAiVersionBookKey => _forceAiVersionBookKey;
  String? get lastCompletedTitle => _lastCompletedTitle;
  String? get lastFailedMessage => _lastFailedMessage;

  /// 测试专用：直接注入任务列表。
  @visibleForTesting
  void debugSetTasks(List<AiTask> tasks) {
    _tasks
      ..clear()
      ..addAll(tasks);
    notifyListeners();
  }

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
    final bookKey = bookKeyOf(source.type, source.id, path);
    if (_tasks.any((t) => t.bookKey == bookKey && t.isActive)) return;
    final t = AiTask(
      id: '${DateTime.now().microsecondsSinceEpoch}',
      bookKey: bookKey,
      sourceType: source.type,
      sourceId: source.id,
      path: path,
      title: title,
      scale: scale,
      sortOrder: _tasks.fold<int>(0, (m, e) => e.sortOrder > m ? e.sortOrder : m) + 1,
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

  /// 拖拽调整排队任务顺序。
  /// [oldIndex]/[newIndex] 是 `activeTasks`（进行中在前）里的下标；进行中任务不可拖。
  Future<void> reorderQueued(int oldIndex, int newIndex) async {
    final active = activeTasks;
    final runCount = active.where((t) => t.status == AiTaskStatus.running).length;
    if (oldIndex < runCount || oldIndex >= active.length) return;
    var ni = newIndex;
    if (ni < runCount) ni = runCount; // 不允许插到进行中任务之前
    if (ni > active.length) ni = active.length;

    final moved = active[oldIndex];
    final rest = active.toList()..removeAt(oldIndex);
    final ordered = <AiTask>[...rest.take(ni), moved, ...rest.skip(ni)];
    _tasks
      ..clear()
      ..addAll(ordered);

    // 按 _tasks 新顺序重编号排队任务（不能再按旧的 sortOrder 排序，否则顺序不变）
    final queued = _tasks.where((t) => t.status == AiTaskStatus.queued).toList();
    for (var i = 0; i < queued.length; i++) {
      queued[i].sortOrder = i + 1;
    }
    final ids = queued.map((t) => t.id).toList();
    try {
      await dbReorderAiTasks(ids: ids);
    } catch (_) {}
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
      final strategy = store.settings.bookOpenStrategy.name;
      final openFuture = switch (source.type) {
        'webdav' => openWebdavBook(
            session: await webdavSessionFor(source),
            path: task.path,
            strategy: strategy),
        'sftp' => openSftpBook(
            session: await sftpSessionFor(source),
            path: task.path,
            strategy: strategy),
        'baidu' => openBaiduBook(
            session: await baiduSessionFor(source),
            path: task.path,
            strategy: strategy),
        '115' => openCloud115Book(
            session: await cloud115SessionFor(source),
            path: task.path,
            strategy: strategy),
        'quark' => openQuarkBook(
            session: await quarkSessionFor(source),
            path: task.path,
            strategy: strategy),
        _ => openLocalBook(path: task.path),
      };
      final bk = await openFuture.timeout(const Duration(seconds: 60));
      task.total = bk.pageCount;
      task.updatedAt = DateTime.now().millisecondsSinceEpoch;
      await _persist(task);
      notifyListeners();

      // 逐页处理：每页完成立即更新进度。
      // 调试构建下 image 编解码较慢（每页约 5-10 秒），
      // 批量模式会让界面长时间停在 0/N，逐页才能恢复流畅的进度递增。
      for (var i = 0; i < task.total; i++) {
        if (task.status == AiTaskStatus.canceled) break;
        try {
          final pageBytes = await bookPage(handle: bk.handle, index: i)
              .timeout(const Duration(seconds: 60));
          final result = await superResolve(pageBytes: pageBytes, scale: task.scale)
              .timeout(const Duration(minutes: 2));
          if (result.isNotEmpty) task.done++;
        } catch (_) {
          // 单页失败：跳过，任务继续
        }
        task.updatedAt = DateTime.now().millisecondsSinceEpoch;
        await _persist(task);
        notifyListeners();
        await _logProgress('进度 ${task.status.name} ${task.done}/${task.total} ${task.title}');
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
