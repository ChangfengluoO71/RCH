import 'package:app/repository/tag_repository.dart';
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/store/tag_provenance.dart';
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
import 'package:flutter/services.dart';

class BookDetailPage extends StatefulWidget {
  final BookSource source;
  final String path;
  final String title;
  const BookDetailPage({
    super.key,
    required this.source,
    required this.path,
    required this.title,
  });
  @override
  State<BookDetailPage> createState() => _BookDetailPageState();
}

class _BookDetailPageState extends State<BookDetailPage> {
  late BookMeta _meta;
  late final TextEditingController _titleCtrl,
      _cnTitleCtrl,
      _authorCtrl,
      _genreCtrl,
      _seriesCtrl,
      _summaryCtrl,
      _commentCtrl;
  int _tagInputKey = 0;

  bool get _bookAiActive => AiUpscaleManager.instance.tasks.any(
    (t) => t.bookKey == _meta.key && t.isActive,
  );

  /// 详情页按钮实时进度（恢复旧版逐页进度显示的体验）。
  String get _aiActiveLabel {
    for (final t in AiUpscaleManager.instance.tasks) {
      if (t.bookKey == _meta.key && t.isActive) {
        return t.total > 0 ? '后台超分中 ${t.done}/${t.total}' : '后台超分中...';
      }
    }
    return '后台超分中...';
  }

  @override
  void initState() {
    super.initState();
    _meta = LibraryStore.instance.metaOf(widget.source, widget.path);
    AiUpscaleManager.instance.addListener(_onAiChanged);
    LibraryStore.instance.addListener(_onProjectionChanged);
    TagRepository.instance.addListener(_onProjectionChanged);
    _titleCtrl = TextEditingController(
      text: _meta.title.isEmpty ? widget.title : _meta.title,
    );
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

  /// Keep the detail projection in sync with read/tag updates made by the
  /// reader, tag manager, or an automation reload while this route remains
  /// mounted underneath another page.
  void _onProjectionChanged() {
    if (!mounted) return;
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

  @override
  void dispose() {
    AiUpscaleManager.instance.removeListener(_onAiChanged);
    LibraryStore.instance.removeListener(_onProjectionChanged);
    TagRepository.instance.removeListener(_onProjectionChanged);
    _titleCtrl.dispose();
    _cnTitleCtrl.dispose();
    _authorCtrl.dispose();
    _genreCtrl.dispose();
    _seriesCtrl.dispose();
    _summaryCtrl.dispose();
    _commentCtrl.dispose();
    super.dispose();
  }

  // ---- 取消 AI 超分并删除缓存 ----

  void _cancelAiSuperResolve() {
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('取消 AI 超分'),
        content: const Text(
          '将移除「AI超分」标签并清空本书的所有 AI 超分缓存。\n\n阅读时将从原始页面加载，可随时重新超分。',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: const Text('返回'),
          ),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: Colors.redAccent),
            onPressed: () async {
              Navigator.of(ctx).pop();
              final store = LibraryStore.instance;
              final bookKey = _meta.key;
              final messenger = ScaffoldMessenger.of(context);
              messenger.showSnackBar(
                const SnackBar(
                  content: Text('正在清除本书 AI 缓存...'),
                  duration: Duration(seconds: 2),
                ),
              );
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
                    strategy: strategy,
                  ),
                  'sftp' => await openSftpBook(
                    session: await sftpSessionFor(s),
                    path: widget.path,
                    strategy: strategy,
                  ),
                  'baidu' => await openBaiduBook(
                    session: await baiduSessionFor(s),
                    path: widget.path,
                    strategy: strategy,
                  ),
                  '115' => await openCloud115BookFor(
                    s,
                    session: await cloud115SessionFor(s),
                    path: widget.path,
                    strategy: strategy,
                  ),
                  'quark' => await openQuarkBook(
                    session: await quarkSessionFor(s),
                    path: widget.path,
                    strategy: strategy,
                  ),
                  _ => await openLocalBook(path: widget.path),
                };
                for (var i = 0; i < bk.pageCount; i++) {
                  final bytes = await bookPage(handle: bk.handle, index: i);
                  await deleteAiCacheForPage(pageBytes: bytes, scale: 2);
                }
                try {
                  closeBook(handle: bk.handle);
                } catch (_) {}
              } catch (e) {
                messenger.showSnackBar(
                  SnackBar(content: Text('清除 AI 缓存失败: $e')),
                );
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
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('已加入后台超分队列，可在右上角悬浮窗查看进度')));
    }
  }

  void _showAiConfirm() {
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('整本 AI 超分'),
        content: const Text(
          '将对本书所有页面执行 2x AI 超分。\n\n• 每页需要 2-5 秒，整本耗时视页数而定\n• 超分结果写入 ai/ 缓存，下次秒开\n• 完成后自动打上「AI超分」元数据标签',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () {
              Navigator.of(ctx).pop();
              _upscaleAll();
            },
            child: const Text('开始超分'),
          ),
        ],
      ),
    );
  }

  /// 置顶的「源:e站」行：点击可隐藏**该漫画**的 E 站导入标签。
  ///
  /// 不用 `removeBookTagsByPrefix`（那个按 bookKey 前缀匹配，语义不同），
  /// 改为逐条 `unlink` 该书的前缀标签——与本页 `_removeTag` 走同一条持久化路径。
  Widget _ehSourceRow(List<String> tags) {
    final scheme = Theme.of(context).colorScheme;
    final color = tagSourceColor(TagSource.ehImport, scheme);
    final count = tags.where((t) => tagSourceOf(t) == TagSource.ehImport).length;
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Row(
        children: [
          InkWell(
            onTap: _hideEhImportedTags,
            borderRadius: BorderRadius.circular(6),
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
              decoration: BoxDecoration(
                color: color.withValues(alpha: 0.16),
                borderRadius: BorderRadius.circular(6),
                border: Border.all(color: color),
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(Icons.public, size: 13, color: color),
                  const SizedBox(width: 4),
                  Text(
                    '$kEhSourceTag  $count',
                    style: TextStyle(fontSize: 12, color: color, fontWeight: FontWeight.w600),
                  ),
                  const SizedBox(width: 4),
                  Icon(Icons.visibility_off_outlined, size: 13, color: color),
                ],
              ),
            ),
          ),
          const SizedBox(width: 8),
          Text('点击隐藏该漫画的 E 站导入标签', style: Theme.of(context).textTheme.bodySmall),
        ],
      ),
    );
  }

  Future<void> _hideEhImportedTags() async {
    final targets = TagRepository.instance
        .tagsForBook(_meta.key)
        .where((t) => tagSourceOf(t) == TagSource.ehImport)
        .toList();
    if (targets.isEmpty) return;
    final ok = await showDialog<bool>(
      context: context,
      builder: (c) => AlertDialog(
        title: const Text('隐藏 E 站导入标签'),
        content: Text(
          '将移除本漫画的 ${targets.length} 个 E 站导入标签（含来源标记）。'
          '自建标签与刮削标签不受影响。移除后可重新导入恢复。',
        ),
        actions: [
          TextButton(onPressed: () => Navigator.of(c).pop(false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.of(c).pop(true), child: const Text('隐藏')),
        ],
      ),
    );
    if (ok != true) return;
    for (final t in targets) {
      TagRepository.instance.unlink(_meta.key, t);
    }
    LibraryStore.instance.saveToDisk();
    if (!mounted) return;
    setState(() {});
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text('已隐藏 ${targets.length} 个 E 站导入标签')),
    );
  }

  /// 影子模式入口：规划一次 E 站元数据导入并**预览**（不写库）。
  ///
  /// 计划来自已落盘的 manifest（离线可复现）：展示将新增的标签、将填补的空白字段、
  /// 以及被跳过的项及原因；用户确认后才执行写入。
  Future<void> _ehImportPreview() async {
    final store = EhSubscriptionStore.instance;
    if (!store.rulesLoaded) await store.init();
    if (!store.hasOutDir) {
      _ehSnack('请先在「设置 → EH 订阅（可选插件）」里选择保存目录并运行一次扫描');
      return;
    }
    final localTags = TagRepository.instance.tagsForBook(_meta.key).toList();
    final snapshot = <String, dynamic>{
      'author': _meta.author,
      'series': _meta.series,
      'summary': _meta.summary,
      'tags': localTags,
    };
    final creators = _meta.author
        .split(RegExp(r'[、,，/]'))
        .map((e) => e.trim())
        .where((e) => e.isNotEmpty)
        .toList();
    final title = _meta.title.isNotEmpty ? _meta.title : widget.title;

    Map<String, dynamic>? plan;
    try {
      plan = await store.planImport(workTitle: title, creators: creators, snapshot: snapshot);
    } catch (e) {
      _ehSnack('规划失败：$e', error: true);
      return;
    }
    if (!mounted) return;
    if (plan == null) {
      _ehSnack('无法规划：请先运行一次 EH 订阅扫描以生成 manifest');
      return;
    }
    await _showImportPlanDialog(plan);
  }

  Future<void> _showImportPlanDialog(Map<String, dynamic> plan) async {
    final status = plan['status'] as String? ?? 'unmatched';
    final tags = (plan['tags'] as List?) ?? const [];
    final fields = (plan['fields'] as List?) ?? const [];
    final skipped = (plan['skipped'] as List?) ?? const [];

    final statusText = switch (status) {
      'matched' => '已匹配到画廊（按${plan['matched_by'] == 'creator' ? '创作者兜底' : '作品名'}）',
      'editions' => '匹配到同一作品的多个版本',
      'ambiguous' => '候选接近或有同系列不同卷，需人工确认（本次不导入）',
      _ => '未匹配到画廊（不猜，本次不导入）',
    };

    final ok = await showDialog<bool>(
      context: context,
      builder: (c) => AlertDialog(
        title: const Text('E 站导入预览（未写入）'),
        content: SizedBox(
          width: 520,
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(statusText, style: const TextStyle(fontWeight: FontWeight.w600)),
                if (plan['gid'] != null)
                  Text('gid: ${plan['gid']}   相似度: '
                      '${(plan['score'] as num?)?.toStringAsFixed(2) ?? '—'}'),
                if ((plan['title_jpn'] as String?)?.isNotEmpty ?? false)
                  Padding(
                    padding: const EdgeInsets.only(top: 2),
                    child: Text(plan['title_jpn'] as String,
                        style: Theme.of(context).textTheme.bodySmall),
                  ),
                const SizedBox(height: 12),
                Text('将新增标签（${tags.length}）', style: const TextStyle(fontWeight: FontWeight.w600)),
                const SizedBox(height: 4),
                if (tags.isEmpty)
                  const Text('（无）', style: TextStyle(fontSize: 12))
                else
                  Wrap(
                    spacing: 6,
                    runSpacing: 4,
                    children: tags
                        .map((t) => TagBox(name: (t as Map)['name'] as String))
                        .toList(),
                  ),
                const SizedBox(height: 12),
                Text('将填补空白字段（${fields.length}）',
                    style: const TextStyle(fontWeight: FontWeight.w600)),
                const SizedBox(height: 4),
                if (fields.isEmpty)
                  const Text('（无）', style: TextStyle(fontSize: 12))
                else
                  ...fields.map((f) => Text(
                        '${(f as Map)['field']} = ${f['value']}',
                        style: const TextStyle(fontSize: 12),
                      )),
                if (skipped.isNotEmpty) ...[
                  const SizedBox(height: 12),
                  Text('已跳过（${skipped.length}）', style: const TextStyle(fontWeight: FontWeight.w600)),
                  const SizedBox(height: 4),
                  ...skipped.map((x) => Text(
                        '· ${(x as Map)['what']}：${x['reason']}',
                        style: Theme.of(context).textTheme.bodySmall,
                      )),
                ],
                const SizedBox(height: 10),
                Text(
                  '说明：字段只填补**空白**项，已有值不会被覆盖（见上方"已跳过"原因）。',
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ],
            ),
          ),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.of(c).pop(false), child: const Text('取消')),
          FilledButton(
            onPressed: (tags.isEmpty && fields.isEmpty) ? null : () => Navigator.of(c).pop(true),
            child: Text('写入 ${tags.length} 个标签 / ${fields.length} 个字段'),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) return;

    var written = 0;
    for (final t in tags) {
      final name = (t as Map)['name'] as String?;
      if (name == null || name.isEmpty) continue;
      TagRepository.instance.link(_meta.key, name);
      written++;
    }
    await TagRepository.instance.persistBookLinks(_meta.key);

    // 字段填补：**只填空**（计划的 fields 里本来就只有空白项），并同步刷新输入框，
    // 保证界面与落库值一致（与 `_saveMeta` 走同一条持久化路径）。
    var filled = 0;
    for (final f in fields) {
      final field = (f as Map)['field'] as String?;
      final value = (f['value'] as String?)?.trim() ?? '';
      if (field == null || value.isEmpty) continue;
      switch (field) {
        case 'author':
          if (_meta.author.trim().isNotEmpty) continue;
          _meta.author = value;
          _authorCtrl.text = value;
        case 'series':
          if (_meta.series.trim().isNotEmpty) continue;
          _meta.series = value;
          _seriesCtrl.text = value;
        case 'summary':
          if (_meta.summary.trim().isNotEmpty) continue;
          _meta.summary = value;
          _summaryCtrl.text = value;
        default:
          continue;
      }
      filled++;
    }
    if (filled > 0) {
      LibraryStore.instance.updateMeta(_meta);
    }
    LibraryStore.instance.saveToDisk();
    if (!mounted) return;
    setState(() {});
    _ehSnack('已写入 $written 个标签、填补 $filled 个字段');
  }

  void _ehSnack(String msg, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(msg),
        backgroundColor: error ? Theme.of(context).colorScheme.error : null,
      ),
    );
  }

  void _addTag(String t) {
    t = t.trim();
    final current = TagRepository.instance.tagsForBook(_meta.key);
    if (t.isEmpty || current.contains(t)) return;
    TagRepository.instance.link(_meta.key, t);
    TagRepository.instance.persistBookLinks(_meta.key);
    LibraryStore.instance.saveToDisk();
    setState(() => _tagInputKey++);
  }

  void _removeTag(String t) {
    TagRepository.instance.unlink(_meta.key, t);
    // saveToDisk performs the repository diff and removes the SQLite link;
    // unlike updateMeta it cannot erase automatically projected tags.
    LibraryStore.instance.saveToDisk();
    setState(() {});
  }

  /// Return the catalog's original entry name, without replacing it with the
  /// user-facing or scraped title.  Paths are persisted with either slash
  /// style depending on their source, so normalize both before taking the
  /// final segment.
  String _originalFilename() {
    final raw = widget.path.trim();
    final fallback = widget.title.trim();
    if (raw.isEmpty) return fallback;
    final normalized = raw.replaceAll('\\', '/');
    final withoutTrailing = normalized.replaceFirst(RegExp(r'/+$'), '');
    final segments = withoutTrailing
        .split('/')
        .map((segment) => segment.trim())
        .where((segment) => segment.isNotEmpty)
        .toList();
    if (segments.isEmpty) return fallback;
    // 2026-09-21（用户报告）：网盘的"容器文件夹/文件夹"路径末段常常是 provider 的
    // **不透明 id** —— 夸克是 32 位 hex，115 是 19 位纯数字（真机 DB 实测
    // `3491122006131214136`、`3502050240473597592`）⇒ 直接显示就是一串哈希/数字。
    // 修法：从末段往前找**第一个不是 id 的段**（`/3491…/3502…/金牌得主` ⇒ 金牌得主）；
    // 整条路径都是 id 时退回目录项名称（各调用点传入的 `title` = `e.name`）。
    for (final segment in segments.reversed) {
      if (!_looksLikeOpaqueProviderId(segment)) return segment;
    }
    return fallback.isEmpty ? segments.last : fallback;
  }

  /// provider 的不透明 id 判定：
  /// · ≥24 位纯 hex（夸克/百度常见的 32 位；客户端 sha256 指纹是 64 位）
  /// · ≥16 位纯数字（115 的 19 位文件/目录 id）
  /// · ≥20 位 `[A-Za-z0-9_-]` 且**没有点**（无扩展名的长 token）
  ///
  /// 真实文件名不会命中：`2.mobi`/`01.zip` 有点，`W-舞冰的祈愿-金牌得主` 含 CJK，
  /// 带空格或括号的名字含非 `[A-Za-z0-9_-]` 字符。
  static final RegExp _opaqueProviderId = RegExp(
    r'^(?:[0-9a-fA-F]{24,}|[0-9]{16,}|[A-Za-z0-9_-]{20,})$',
  );

  static bool _looksLikeOpaqueProviderId(String value) =>
      _opaqueProviderId.hasMatch(value);

  Future<void> _copyOriginalFilename(String filename) async {
    if (filename.isEmpty) return;
    await Clipboard.setData(ClipboardData(text: filename));
    if (!mounted) return;
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(const SnackBar(content: Text('原文件名已复制')));
  }

  Widget _identifiedMetaLine(String label, String value) => Padding(
    padding: const EdgeInsets.only(bottom: 3),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          width: 58,
          child: Text('$label：', style: const TextStyle(color: Colors.white60)),
        ),
        Expanded(child: Text(value)),
      ],
    ),
  );

  Widget _originalFilenameLine(String filename) => Padding(
    padding: const EdgeInsets.only(bottom: 3),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(
          width: 72,
          child: Text('原文件名：', style: TextStyle(color: Colors.white60)),
        ),
        Expanded(child: SelectableText(filename)),
        IconButton(
          key: const Key('copy_original_filename'),
          tooltip: '复制原文件名',
          icon: const Icon(Icons.copy, size: 18),
          padding: EdgeInsets.zero,
          constraints: const BoxConstraints(minWidth: 32, minHeight: 32),
          onPressed: filename.isEmpty
              ? null
              : () => _copyOriginalFilename(filename),
        ),
      ],
    ),
  );

  Widget _metaField(String label, TextEditingController ctrl) => Padding(
    padding: const EdgeInsets.only(bottom: 8),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Icon(Icons.label, size: 16, color: Colors.redAccent.shade200),
            const SizedBox(width: 4),
            Text(
              label,
              style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 13),
            ),
          ],
        ),
        const SizedBox(height: 4),
        TextField(
          controller: ctrl,
          decoration: const InputDecoration(
            border: OutlineInputBorder(),
            isDense: true,
          ),
          onChanged: (_) => _saveMeta(),
        ),
      ],
    ),
  );

  @override
  Widget build(BuildContext context) {
    final record = LibraryStore.instance.recordOf(widget.source, widget.path);
    final bookKey = _meta.key;
    final hasReadTag = TagRepository.instance
        .bookKeysForTag('已读')
        .contains(bookKey);
    final hasAiTag = TagRepository.instance
        .bookKeysForTag('AI超分')
        .contains(bookKey);
    final identifiedTitle = _meta.title.isNotEmpty ? _meta.title : widget.title;
    final originalFilename = _originalFilename();
    // Metadata fields are canonical projections as well as tag-manager
    // entries. Merge both sources so details remain readable even when an old
    // database has not yet rebuilt its derived tag links.
    final identifiedTags = <String>{
      ..._meta.metaTags,
      ...TagRepository.instance.tagsForBook(bookKey),
    }.toList();
    return Scaffold(
      appBar: AppBar(title: const Text('漫画详情')),
      body: LayoutBuilder(
        builder: (context, box) {
          final compact = box.maxWidth < 600;
          final headerWidgets = <Widget>[
            if (widget.source.remoteOnly) ...[
              Container(
                width: 220,
                height: 310,
                decoration: BoxDecoration(
                  color: Theme.of(context).colorScheme.surfaceContainerHighest,
                  borderRadius: BorderRadius.circular(8),
                ),
                child: Center(
                  child: Text(
                    '仅元数据\n来自${SyncManager.instance.deviceNameOf(widget.source.originDeviceId)}',
                    textAlign: TextAlign.center,
                    style: TextStyle(color: Theme.of(context).colorScheme.onSurfaceVariant, fontSize: 13),
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
              SizedBox(
                width: 220,
                height: 310,
                child: ComicCover(
                  source: widget.source,
                  path: widget.path,
                  force: true,
                ),
              ),
              const SizedBox(height: 16),
              SizedBox(
                width: 220,
                child: FilledButton.icon(
                  onPressed: () => openBook(
                    context,
                    widget.source,
                    widget.path,
                    widget.title,
                  ),
                  icon: const Icon(Icons.menu_book),
                  label: const Text('开始阅读'),
                ),
              ),
              const SizedBox(height: 8),
              SizedBox(
                width: 220,
                child: OutlinedButton.icon(
                  onPressed: () async {
                    await Navigator.of(context).push(
                      MaterialPageRoute(
                        builder: (_) => CoverEditorPage(
                          source: widget.source,
                          path: widget.path,
                          title: widget.title,
                        ),
                      ),
                    );
                    setState(() {});
                  },
                  icon: const Icon(Icons.crop),
                  label: const Text('自定义封面'),
                ),
              ),
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
                icon: Icon(
                  hasReadTag
                      ? Icons.check_circle
                      : Icons.radio_button_unchecked,
                  size: 18,
                  color: hasReadTag ? Colors.redAccent : Colors.grey,
                ),
                label: Text(
                  hasReadTag ? '已读' : '标记已读',
                  style: TextStyle(color: hasReadTag ? Colors.redAccent : null),
                ),
              ),
            ),
            // 整本 AI 超分（幽灵书源无源文件，隐藏）
            if (!widget.source.remoteOnly && !isAndroidPlatform) ...[
              SizedBox(
                width: 220,
                child: _bookAiActive
                    ? OutlinedButton.icon(
                        onPressed: null,
                        icon: const SizedBox(
                          width: 16,
                          height: 16,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        ),
                        label: Text(_aiActiveLabel),
                      )
                    : hasAiTag
                    ? OutlinedButton.icon(
                        onPressed: _showAiConfirm,
                        icon: const Icon(
                          Icons.auto_fix_high,
                          size: 18,
                          color: Colors.purple,
                        ),
                        label: const Text(
                          '重新 AI 超分',
                          style: TextStyle(color: Colors.purple),
                        ),
                      )
                    : OutlinedButton.icon(
                        onPressed: _showAiConfirm,
                        icon: const Icon(Icons.auto_fix_high, size: 18),
                        label: const Text('整本 AI 超分'),
                      ),
              ),
              // 取消 AI 超分并删除缓存
              if (hasAiTag) ...[
                SizedBox(
                  width: 220,
                  child: OutlinedButton.icon(
                    onPressed: _cancelAiSuperResolve,
                    icon: const Icon(
                      Icons.delete_outline,
                      size: 18,
                      color: Colors.redAccent,
                    ),
                    label: const Text(
                      '取消 AI 超分',
                      style: TextStyle(color: Colors.redAccent),
                    ),
                  ),
                ),
                SizedBox(
                  width: 220,
                  child: OutlinedButton.icon(
                    onPressed: () => openBookNoAi(
                      context,
                      widget.source,
                      widget.path,
                      widget.title,
                    ),
                    icon: const Icon(Icons.hide_image, size: 18),
                    label: const Text('阅读未超分版本'),
                  ),
                ),
              ],
            ],
          ];
          final infoWidgets = <Widget>[
            Text(
              identifiedTitle,
              style: const TextStyle(fontSize: 18, fontWeight: FontWeight.bold),
            ),
            const SizedBox(height: 8),
            if (_meta.chineseTitle.isNotEmpty)
              _identifiedMetaLine('中文标题', _meta.chineseTitle),
            if (_meta.author.isNotEmpty)
              _identifiedMetaLine('作者', _meta.author),
            if (_meta.series.isNotEmpty)
              _identifiedMetaLine('系列', _meta.series),
            if (_meta.genre.isNotEmpty) _identifiedMetaLine('类别', _meta.genre),
            if (originalFilename.isNotEmpty)
              _originalFilenameLine(originalFilename),
            if (identifiedTags.isNotEmpty) ...[
              const SizedBox(height: 4),
              if (hasEhImportedTags(identifiedTags)) _ehSourceRow(identifiedTags.toList()),
              Wrap(
                spacing: 6,
                runSpacing: 4,
                children: identifiedTags.map((tag) => TagBox(name: tag)).toList(),
              ),
            ],
            const SizedBox(height: 12),
            if (record != null)
              Text(
                '阅读进度:第 ${record.lastPage + 1} 页 · 看过 ${record.readCount} 次',
                style: Theme.of(context).textTheme.bodySmall,
              ),
            const SizedBox(height: 20),
            Row(
              children: [
                const Text(
                  '元数据标签',
                  style: TextStyle(fontWeight: FontWeight.w600, fontSize: 14),
                ),
                const Spacer(),
                TextButton.icon(
                  onPressed: _ehImportPreview,
                  icon: const Icon(Icons.cloud_download_outlined, size: 18),
                  label: const Text('从 E 站导入'),
                ),
              ],
            ),
            const SizedBox(height: 4),
            Text(
              '作者/类别/系列用于管理和检索',
              style: Theme.of(context).textTheme.bodySmall,
            ),
            const SizedBox(height: 8),
            _metaField('标题', _titleCtrl),
            _metaField('中文标题', _cnTitleCtrl),
            _metaField('作者', _authorCtrl),
            _metaField('类别', _genreCtrl),
            _metaField('系列', _seriesCtrl),
            const SizedBox(height: 8),
            const Text(
              '标签',
              style: TextStyle(fontWeight: FontWeight.w600, fontSize: 14),
            ),
            const SizedBox(height: 6),
            ListenableBuilder(
              listenable: TagRepository.instance,
              builder: (context, _) {
                final all = LibraryStore.instance.allTags();
                final bookTags = TagRepository.instance.tagsForBook(bookKey);
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Autocomplete<String>(
                      key: ValueKey('tag_input_$_tagInputKey'),
                      optionsBuilder: (v) {
                        final q = v.text.toLowerCase();
                        if (q.isEmpty) return const <String>[];
                        return all
                            .where((t) => t.toLowerCase().contains(q))
                            .toList();
                      },
                      fieldViewBuilder: (context, ctrl, fn, _) {
                        void commit(String val) {
                          final t = val.trim();
                          if (t.isNotEmpty) _addTag(t);
                          ctrl.clear();
                        }

                        return TextField(
                          controller: ctrl,
                          focusNode: fn,
                          decoration: const InputDecoration(
                            hintText: '输入标签',
                            isDense: true,
                            border: OutlineInputBorder(),
                          ),
                          onSubmitted: commit,
                          onTapOutside: (_) => fn.unfocus(),
                          onEditingComplete: () => commit(ctrl.text),
                        );
                      },
                      onSelected: (String sel) {
                        _addTag(sel);
                      },
                    ),
                    const SizedBox(height: 8),
                    if (bookTags.isNotEmpty)
                      Wrap(
                        spacing: 8,
                        runSpacing: 4,
                        children: bookTags
                            .map((t) => TagBox(name: t, onDeleted: () => _removeTag(t)))
                            .toList(),
                      ),
                  ],
                );
              },
            ),
            const SizedBox(height: 20),
            const Text('简介', style: TextStyle(fontWeight: FontWeight.w600)),
            const SizedBox(height: 8),
            TextField(
              controller: _summaryCtrl,
              maxLines: 4,
              decoration: const InputDecoration(border: OutlineInputBorder()),
              onChanged: (_) => _saveMeta(),
            ),
            const SizedBox(height: 20),
            const Text('感想', style: TextStyle(fontWeight: FontWeight.w600)),
            const SizedBox(height: 8),
            TextField(
              controller: _commentCtrl,
              maxLines: 4,
              decoration: const InputDecoration(border: OutlineInputBorder()),
              onChanged: (_) => _saveMeta(),
            ),
          ];
          final header = Padding(
            padding: const EdgeInsets.all(20),
            // 矮屏（如安卓横屏逻辑高 480dp）下封面列高度可能超出视口，
            // 包一层滚动避免 RenderFlex 底部溢出（黄黑报错条遮挡按钮）。
            child: SingleChildScrollView(
              child: Column(children: headerWidgets),
            ),
          );
          final info = ListView(
            padding: const EdgeInsets.all(20),
            children: infoWidgets,
          );
          if (!compact) {
            return Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                header,
                const VerticalDivider(width: 1),
                Expanded(child: info),
              ],
            );
          }
          return ListView(
            padding: const EdgeInsets.all(16),
            children: [header, const Divider(height: 24), ...infoWidgets],
          );
        },
      ),
    );
  }
}
