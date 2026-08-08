import 'package:app/repository/tag_repository.dart';
import 'package:app/store/ai_upscale_manager.dart';
import 'package:app/store/baidu_session.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/quark_session.dart';
import 'package:app/store/sftp_session.dart';
import 'package:app/store/sync_manager.dart';
import 'package:app/store/webdav_session.dart';
import 'package:app/src/rust/api/ai.dart';
import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/common.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:app/ui/cover_editor_page.dart';
import 'package:app/ui/opener.dart';
import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';

class BookDetailPage extends StatefulWidget {
  final BookSource source;
  final String path;
  final String title;
  const BookDetailPage({super.key, required this.source, required this.path, required this.title});
  @override State<BookDetailPage> createState() => _BookDetailPageState();
}

class _BookDetailPageState extends State<BookDetailPage> {
  late BookMeta _meta;
  late final TextEditingController _titleCtrl, _cnTitleCtrl, _authorCtrl, _genreCtrl, _seriesCtrl, _summaryCtrl, _commentCtrl;
  int _tagInputKey = 0;

  bool get _bookAiActive =>
      AiUpscaleManager.instance.tasks.any((t) => t.bookKey == _meta.key && t.isActive);

  /// 详情页按钮实时进度（恢复旧版逐页进度显示的体验）。
  String get _aiActiveLabel {
    for (final t in AiUpscaleManager.instance.tasks) {
      if (t.bookKey == _meta.key && t.isActive) {
        return t.total > 0 ? '后台超分中 ${t.done}/${t.total}' : '后台超分中...';
      }
    }
    return '后台超分中...';
  }

  @override void initState() { super.initState();
    _meta = LibraryStore.instance.metaOf(widget.source, widget.path);
    AiUpscaleManager.instance.addListener(_onAiChanged);
    _titleCtrl = TextEditingController(text: _meta.title.isEmpty ? widget.title : _meta.title);
    _cnTitleCtrl = TextEditingController(text: _meta.chineseTitle);
    _authorCtrl = TextEditingController(text: _meta.author);
    _genreCtrl = TextEditingController(text: _meta.genre);
    _seriesCtrl = TextEditingController(text: _meta.series);
    _summaryCtrl = TextEditingController(text: _meta.summary);
    _commentCtrl = TextEditingController(text: _meta.comment);
  }

  void _onAiChanged() {
    if (!mounted) return;
    // 若在构建/布局帧内收到通知（例如 ReaderPage 挂载时 AI 管理器 notify），
    // 延迟到帧结束后再刷新，避免 "setState() called during build"。
    final phase = SchedulerBinding.instance.schedulerPhase;
    if (phase == SchedulerPhase.persistentCallbacks ||
        phase == SchedulerPhase.midFrameMicrotasks) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) setState(() {});
      });
      return;
    }
    setState(() {});
  }

  @override void dispose() {
    AiUpscaleManager.instance.removeListener(_onAiChanged);
    _titleCtrl.dispose(); _cnTitleCtrl.dispose(); _authorCtrl.dispose(); _genreCtrl.dispose(); _seriesCtrl.dispose(); _summaryCtrl.dispose(); _commentCtrl.dispose(); super.dispose();
  }

  // ---- 取消 AI 超分并删除缓存 ----

  void _cancelAiSuperResolve() {
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('取消 AI 超分'),
        content: const Text('将移除「AI超分」标签并清空本书的所有 AI 超分缓存。\n\n阅读时将从原始页面加载，可随时重新超分。'),
        actions: [
          TextButton(onPressed: () => Navigator.of(ctx).pop(), child: const Text('返回')),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: Colors.redAccent),
            onPressed: () async {
              Navigator.of(ctx).pop();
              final store = LibraryStore.instance;
              final bookKey = _meta.key;
              final messenger = ScaffoldMessenger.of(context);
              messenger.showSnackBar(const SnackBar(content: Text('正在清除本书 AI 缓存...'), duration: Duration(seconds: 2)));
              // 移除 AI超分 标签
              TagRepository.instance.unlink(bookKey, 'AI超分');
              await store.saveToDisk();
              // 只清本书的 AI 缓存：逐页按内容 hash 删除，不影响其他书
              try {
                final s = widget.source;
                final strategy = store.settings.bookOpenStrategy.name;
                final bk = switch (s.type) {
                  'webdav' => await openWebdavBook(
                      session: await webdavSessionFor(s),
                      path: widget.path,
                      strategy: strategy),
                  'sftp' => await openSftpBook(
                      session: await sftpSessionFor(s),
                      path: widget.path,
                      strategy: strategy),
                  'baidu' => await openBaiduBook(
                      session: await baiduSessionFor(s),
                      path: widget.path,
                      strategy: strategy),
                  '115' => await openCloud115BookFor(s,
                      session: await cloud115SessionFor(s),
                      path: widget.path,
                      strategy: strategy),
                  'quark' => await openQuarkBook(
                      session: await quarkSessionFor(s),
                      path: widget.path,
                      strategy: strategy),
                  _ => await openLocalBook(path: widget.path),
                };
                for (var i = 0; i < bk.pageCount; i++) {
                  final bytes = await bookPage(handle: bk.handle, index: i);
                  await deleteAiCacheForPage(pageBytes: bytes, scale: 2);
                }
                try { closeBook(handle: bk.handle); } catch (_) {}
              } catch (e) {
                messenger.showSnackBar(SnackBar(content: Text('清除 AI 缓存失败: $e')));
              }
              if (mounted) setState(() {});
            },
            child: const Text('确认取消'),
          ),
        ],
      ),
    );
  }

  void _saveMeta() {
    _meta.title = _titleCtrl.text.trim();
    _meta.chineseTitle = _cnTitleCtrl.text.trim();
    _meta.author = _authorCtrl.text.trim();
    _meta.genre = _genreCtrl.text.trim();
    _meta.series = _seriesCtrl.text.trim();
    _meta.summary = _summaryCtrl.text.trim();
    _meta.comment = _commentCtrl.text.trim();
    LibraryStore.instance.updateMeta(_meta);
  }

  // ---- 整本 AI 超分 ----

  Future<void> _upscaleAll() async {
    await AiUpscaleManager.instance.enqueue(
      source: widget.source,
      path: widget.path,
      title: _meta.title.isEmpty ? widget.title : _meta.title,
      scale: 2,
    );
    if (mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('已加入后台超分队列，可在右上角悬浮窗查看进度')),
      );
    }
  }

  void _showAiConfirm() {
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('整本 AI 超分'),
        content: const Text('将对本书所有页面执行 2x AI 超分。\n\n• 每页需要 2-5 秒，整本耗时视页数而定\n• 超分结果写入 ai/ 缓存，下次秒开\n• 完成后自动打上「AI超分」元数据标签'),
        actions: [
          TextButton(onPressed: () => Navigator.of(ctx).pop(), child: const Text('取消')),
          FilledButton(onPressed: () { Navigator.of(ctx).pop(); _upscaleAll(); }, child: const Text('开始超分')),
        ],
      ),
    );
  }

  void _addTag(String t) { t = t.trim(); if (t.isEmpty || _meta.tags.contains(t)) return; setState(() => _meta.tags.add(t)); LibraryStore.instance.updateMeta(_meta); setState(() => _tagInputKey++); }
  void _removeTag(String t) { setState(() => _meta.tags.remove(t)); LibraryStore.instance.updateMeta(_meta); }

  Widget _metaField(String label, TextEditingController ctrl) => Padding(
    padding: const EdgeInsets.only(bottom: 8),
    child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Row(children: [Icon(Icons.label, size: 16, color: Colors.redAccent.shade200), const SizedBox(width: 4), Text(label, style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 13))]),
      const SizedBox(height: 4),
      TextField(controller: ctrl, decoration: const InputDecoration(border: OutlineInputBorder(), isDense: true), onChanged: (_) => _saveMeta()),
    ]),
  );

  @override Widget build(BuildContext context) {
    final record = LibraryStore.instance.recordOf(widget.source, widget.path);
    final bookKey = bookKeyOf(widget.source.type, widget.source.id, widget.path);
    final hasReadTag = TagRepository.instance.bookKeysForTag('已读').contains(bookKey);
    final hasAiTag = TagRepository.instance.bookKeysForTag('AI超分').contains(bookKey);
    return Scaffold(
      appBar: AppBar(title: const Text('漫画详情')),
      body: LayoutBuilder(builder: (context, box) {
        final compact = box.maxWidth < 600;
        final headerWidgets = <Widget>[
          if (widget.source.remoteOnly) ...[
            Container(
              width: 220, height: 310,
              decoration: BoxDecoration(
                color: Colors.white10,
                borderRadius: BorderRadius.circular(8),
              ),
              child: Center(
                child: Text(
                  '仅元数据\n来自${SyncManager.instance.deviceNameOf(widget.source.originDeviceId)}',
                  textAlign: TextAlign.center,
                  style: const TextStyle(color: Colors.white54, fontSize: 13),
                ),
              ),
            ),
            const SizedBox(height: 16),
            SizedBox(
              width: 220,
              child: FilledButton.icon(
                onPressed: null,
                icon: const Icon(Icons.lock_outline),
                label: const Text('其他设备书源，不可阅读'),
              ),
            ),
          ] else ...[
            SizedBox(width: 220, height: 310, child: ComicCover(source: widget.source, path: widget.path, force: true)),
            const SizedBox(height: 16),
            SizedBox(width: 220, child: FilledButton.icon(onPressed: () => openBook(context, widget.source, widget.path, widget.title), icon: const Icon(Icons.menu_book), label: const Text('开始阅读'))),
            const SizedBox(height: 8),
            SizedBox(width: 220, child: OutlinedButton.icon(onPressed: () async { await Navigator.of(context).push(MaterialPageRoute(builder: (_) => CoverEditorPage(source: widget.source, path: widget.path, title: widget.title))); setState(() {}); }, icon: const Icon(Icons.crop), label: const Text('自定义封面'))),
          ],
          const SizedBox(height: 8),
          // 已读/未读切换按钮
          SizedBox(
            width: 220,
            child: OutlinedButton.icon(
              onPressed: () {
                setState(() {
                  if (hasReadTag) {
                    TagRepository.instance.unlink(bookKey, '已读');
                  } else {
                    TagRepository.instance.link(bookKey, '已读');
                  }
                });
                LibraryStore.instance.saveToDisk();
              },
              icon: Icon(hasReadTag ? Icons.check_circle : Icons.radio_button_unchecked,
                  size: 18, color: hasReadTag ? Colors.redAccent : Colors.grey),
              label: Text(hasReadTag ? '已读' : '标记已读',
                  style: TextStyle(color: hasReadTag ? Colors.redAccent : null)),
            ),
          ),
          // 整本 AI 超分（幽灵书源无源文件，隐藏）
          if (!widget.source.remoteOnly && !isAndroidPlatform) ...[
            SizedBox(width: 220, child: _bookAiActive
              ? OutlinedButton.icon(
                  onPressed: null,
                  icon: const SizedBox(width: 16, height: 16, child: CircularProgressIndicator(strokeWidth: 2)),
                  label: Text(_aiActiveLabel),
                )
              : hasAiTag
                ? OutlinedButton.icon(
                    onPressed: _showAiConfirm,
                    icon: const Icon(Icons.auto_fix_high, size: 18, color: Colors.purple),
                    label: const Text('重新 AI 超分', style: TextStyle(color: Colors.purple)),
                  )
                : OutlinedButton.icon(
                    onPressed: _showAiConfirm,
                    icon: const Icon(Icons.auto_fix_high, size: 18),
                    label: const Text('整本 AI 超分'),
                  ),
            ),
            // 取消 AI 超分并删除缓存
            if (hasAiTag) ...[
              SizedBox(width: 220, child: OutlinedButton.icon(
                onPressed: _cancelAiSuperResolve,
                icon: const Icon(Icons.delete_outline, size: 18, color: Colors.redAccent),
                label: const Text('取消 AI 超分', style: TextStyle(color: Colors.redAccent)),
              )),
              SizedBox(width: 220, child: OutlinedButton.icon(
                onPressed: () => openBookNoAi(context, widget.source, widget.path, widget.title),
                icon: const Icon(Icons.hide_image, size: 18),
                label: const Text('阅读未超分版本'),
              )),
            ],
          ],
        ];
        final infoWidgets = <Widget>[
          Text(widget.title, style: const TextStyle(fontSize: 18, fontWeight: FontWeight.bold)),
          const SizedBox(height: 8),
          if (record != null) Text('阅读进度:第 ${record.lastPage + 1} 页 · 看过 ${record.readCount} 次', style: Theme.of(context).textTheme.bodySmall),
          const SizedBox(height: 20),
          const Text('元数据标签', style: TextStyle(fontWeight: FontWeight.w600, fontSize: 14)),
          const SizedBox(height: 4),
          Text('作者/类别/系列用于管理和检索', style: Theme.of(context).textTheme.bodySmall),
          const SizedBox(height: 8),
          _metaField('标题', _titleCtrl),
          _metaField('中文标题', _cnTitleCtrl),
          _metaField('作者', _authorCtrl),
          _metaField('类别', _genreCtrl),
          _metaField('系列', _seriesCtrl),
          const SizedBox(height: 8),
          const Text('标签', style: TextStyle(fontWeight: FontWeight.w600, fontSize: 14)),
          const SizedBox(height: 6),
          ListenableBuilder(listenable: LibraryStore.instance, builder: (context, _) {
            final all = LibraryStore.instance.allTags();
            return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Autocomplete<String>(
                key: ValueKey('tag_input_$_tagInputKey'),
                optionsBuilder: (v) { final q = v.text.toLowerCase(); if (q.isEmpty) return const <String>[]; return all.where((t) => t.toLowerCase().contains(q)).toList(); },
                fieldViewBuilder: (context, ctrl, fn, _) {
                  void commit(String val) { final t = val.trim(); if (t.isNotEmpty) _addTag(t); ctrl.clear(); }
                  return TextField(controller: ctrl, focusNode: fn, decoration: const InputDecoration(hintText: '输入标签', isDense: true, border: OutlineInputBorder()), onSubmitted: commit, onTapOutside: (_) => fn.unfocus(), onEditingComplete: () => commit(ctrl.text));
                },
                onSelected: (String sel) { _addTag(sel); },
              ),
              const SizedBox(height: 8),
              if (_meta.tags.isNotEmpty) Wrap(spacing: 8, runSpacing: 4, children: _meta.tags.map((t) => Chip(label: Text(t), onDeleted: () => _removeTag(t))).toList()),
            ]);
          }),
          const SizedBox(height: 20),
          const Text('简介', style: TextStyle(fontWeight: FontWeight.w600)),
          const SizedBox(height: 8),
          TextField(controller: _summaryCtrl, maxLines: 4, decoration: const InputDecoration(border: OutlineInputBorder()), onChanged: (_) => _saveMeta()),
          const SizedBox(height: 20),
          const Text('感想', style: TextStyle(fontWeight: FontWeight.w600)),
          const SizedBox(height: 8),
          TextField(controller: _commentCtrl, maxLines: 4, decoration: const InputDecoration(border: OutlineInputBorder()), onChanged: (_) => _saveMeta()),
        ];
        final header = Padding(
          padding: const EdgeInsets.all(20),
          // 矮屏（如安卓横屏逻辑高 480dp）下封面列高度可能超出视口，
          // 包一层滚动避免 RenderFlex 底部溢出（黄黑报错条遮挡按钮）。
          child: SingleChildScrollView(
            child: Column(children: headerWidgets),
          ),
        );
        final info = ListView(padding: const EdgeInsets.all(20), children: infoWidgets);
        if (!compact) {
          return Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
            header,
            const VerticalDivider(width: 1),
            Expanded(child: info),
          ]);
        }
        return ListView(
          padding: const EdgeInsets.all(16),
          children: [
            header,
            const Divider(height: 24),
            ...infoWidgets,
          ],
        );
      }),
    );
  }
}
