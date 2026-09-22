import 'dart:convert';

import 'package:app/repository/tag_repository.dart';
import 'package:app/src/rust/api/scraper.dart' as scraperapi;
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/tag_provenance.dart';
import 'package:flutter/material.dart';

/// 设置页「E 站自动刮削」：选择一个书源的某个文件夹，批量识别并导入 E 站元数据。
///
/// 设计取舍：
/// * **工作清单取自 M8 刮削的 proposals**（`dbLoadScrapeProposals`）——既拿到刮削解析出的
///   干净作品名（匹配输入），又天然限定在"已刮削过的书"，不需要再枚举远端目录。
/// * **批量在 Dart 侧串行驱动**，每一本调用已有的 `eh_plan_book_live`：
///   进度/取消都在 UI 侧，避免为批处理再造一套 Rust 进度与取消机制。
/// * **自动写入仅限"作品名命中且唯一"**；创作者兜底、同系列不同卷、并列接近一律只列出待人工确认
///   （与既有"不许猜"口径一致）。
class EhAutoScrapePanel extends StatefulWidget {
  const EhAutoScrapePanel({super.key});

  @override
  State<EhAutoScrapePanel> createState() => _EhAutoScrapePanelState();
}

/// 一条批量结果。
class _BatchRow {
  _BatchRow(this.bookKey, this.title);

  final String bookKey;
  final String title;
  Map<String, dynamic>? plan;
  String? error;
  bool get matchedUnique =>
      plan?['status'] == 'matched' && plan?['matched_by'] == 'title';
  String get statusLabel {
    if (error != null) return '失败';
    final s = plan?['status'] as String? ?? '—';
    return switch (s) {
      'matched' => plan?['matched_by'] == 'creator' ? '创作者兜底（待确认）' : '命中（可自动写）',
      'editions' => '同一作品多版本',
      'ambiguous' => '需人工确认',
      _ => '未匹配',
    };
  }
}

class _EhAutoScrapePanelState extends State<EhAutoScrapePanel> {
  String? _sourceId;
  final _folderCtrl = TextEditingController();
  final _limitCtrl = TextEditingController(text: '50');
  int _limit = 50;
  bool _running = false;
  bool _cancel = false;
  int _done = 0;
  int _total = 0;
  int _autoWritten = 0;
  final List<_BatchRow> _rows = [];

  @override
  void dispose() {
    _folderCtrl.dispose();
    _limitCtrl.dispose();
    super.dispose();
  }

  Future<List<_BatchRow>> _collectWorkItems() async {
    final props = await scraperapi.dbLoadScrapeProposals(limit: 100000, state: 'ready');
    final prefix = _folderCtrl.text.trim();
    final rows = <_BatchRow>[];
    for (final p in props) {
      if (_sourceId != null && p.sourceId != _sourceId) continue;
      if (prefix.isNotEmpty && !p.path.startsWith(prefix)) continue;
      final sem = (jsonDecode(p.semanticJson) as Map?) ?? const {};
      final title = (sem['work_title'] as String?)?.trim() ?? '';
      final fallback = p.title?.trim() ?? p.filename;
      rows.add(_BatchRow(p.bookKey, title.isNotEmpty ? title : fallback));
    }
    return rows;
  }

  List<String> _creatorsOf(String bookKey, String semanticJson) {
    try {
      final sem = (jsonDecode(semanticJson) as Map?) ?? const {};
      final out = <String>[];
      for (final c in (sem['creators'] as List?) ?? const []) {
        final n = ((c as Map)['name'] as String?)?.trim() ?? '';
        if (n.isNotEmpty && !out.contains(n)) out.add(n);
      }
      return out;
    } catch (_) {
      return const [];
    }
  }

  Future<void> _run() async {
    final store = EhSubscriptionStore.instance;
    if (!store.rulesLoaded) await store.init();
    if (!store.hasOutDir) {
      _snack('请先在「EH 订阅（可选插件）」里选择保存目录', error: true);
      return;
    }
    final items = await _collectWorkItems();
    if (items.isEmpty) {
      _snack('该范围内没有可识别的刮削结果（先在「智能刮削」里跑一轮）', error: true);
      return;
    }
    final todo = items.take(_limit).toList();
    final props = await scraperapi.dbLoadScrapeProposals(limit: 100000, state: 'ready');
    final byKey = {for (final p in props) p.bookKey: p};

    setState(() {
      _rows
        ..clear()
        ..addAll(todo);
      _running = true;
      _cancel = false;
      _done = 0;
      _total = todo.length;
      _autoWritten = 0;
    });

    for (final row in _rows) {
      if (_cancel) break;
      if (!mounted) return;
      try {
        final p = byKey[row.bookKey];
        final creators = p == null ? <String>[] : _creatorsOf(p.bookKey, p.semanticJson);
        final meta = LibraryStore.instance.metas[row.bookKey];
        final snapshot = <String, dynamic>{
          'author': meta?.author ?? '',
          'series': meta?.series ?? '',
          'summary': meta?.summary ?? '',
          'tags': TagRepository.instance.tagsForBook(row.bookKey),
        };
        final plan = await store.planImportLive(
          workTitle: row.title,
          creators: creators,
          snapshot: snapshot,
        );
        row.plan = plan;
        // 只对"作品名命中且唯一"自动写
        if (row.matchedUnique && plan != null) {
          await _applyPlan(row.bookKey, plan);
          _autoWritten++;
        }
      } catch (e) {
        row.error = '$e';
      }
      if (!mounted) return;
      setState(() => _done++);
    }

    if (!mounted) return;
    setState(() => _running = false);
    _snack('本轮完成：$_done 本，自动写入 $_autoWritten 本（其余待确认）');
  }

  /// 按计划写入标签与空白字段（与详情页导入同一条路径）。
  Future<void> _applyPlan(String bookKey, Map<String, dynamic> plan) async {
    for (final t in (plan['tags'] as List?) ?? const []) {
      final name = (t as Map)['name'] as String?;
      if (name == null || name.isEmpty) continue;
      TagRepository.instance.link(bookKey, name);
    }
    await TagRepository.instance.persistBookLinks(bookKey);
    final meta = LibraryStore.instance.metas[bookKey];
    if (meta != null) {
      var changed = false;
      for (final f in (plan['fields'] as List?) ?? const []) {
        final field = (f as Map)['field'] as String?;
        final value = (f['value'] as String?)?.trim() ?? '';
        if (value.isEmpty) continue;
        switch (field) {
          case 'author':
            if (meta.author.trim().isEmpty) {
              meta.author = value;
              changed = true;
            }
          case 'series':
            if (meta.series.trim().isEmpty) {
              meta.series = value;
              changed = true;
            }
        }
      }
      if (changed) LibraryStore.instance.updateMeta(meta);
    }
    LibraryStore.instance.saveToDisk();
  }

  void _snack(String msg, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(msg),
        backgroundColor: error ? Theme.of(context).colorScheme.error : null,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final sources = LibraryStore.instance.sources;
    return ListenableBuilder(
      listenable: LibraryStore.instance,
      builder: (context, _) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('E 站自动刮削',
              style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)),
          const SizedBox(height: 4),
          Text(
            '选择一个书源的某个文件夹作为 E 站自动刮削目录：对该范围内**已刮削**的书，'
            '用刮削解析出的作品名实时检索 E 站并导入标签与元数据。'
            '只有"作品名命中且唯一"会自动写入，其余列出来等你确认。',
            style: theme.textTheme.bodySmall,
          ),
          const SizedBox(height: 12),
          Row(
            children: [
              Expanded(
                flex: 3,
                child: DropdownButtonFormField<String>(
                  initialValue: _sourceId,
                  decoration: const InputDecoration(
                    labelText: '书源',
                    isDense: true,
                    border: OutlineInputBorder(),
                  ),
                  items: [
                    for (final s in sources)
                      DropdownMenuItem(value: s.id, child: Text(s.name)),
                  ],
                  onChanged: _running ? null : (v) => setState(() => _sourceId = v),
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                flex: 4,
                child: TextField(
                  controller: _folderCtrl,
                  enabled: !_running,
                  decoration: const InputDecoration(
                    labelText: '文件夹前缀（留空 = 该书源全部）',
                    hintText: '如 日漫 或 日漫/合订',
                    isDense: true,
                    border: OutlineInputBorder(),
                  ),
                ),
              ),
              const SizedBox(width: 12),
              SizedBox(
                width: 110,
                child: TextField(
                  enabled: !_running,
                  keyboardType: TextInputType.number,
                  decoration: const InputDecoration(
                    labelText: '每轮上限',
                    isDense: true,
                    border: OutlineInputBorder(),
                  ),
                  controller: _limitCtrl,
                  onChanged: (v) => _limit = (int.tryParse(v.trim()) ?? 50).clamp(1, 5000),
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          Row(
            children: [
              FilledButton.icon(
                onPressed: _running ? null : _run,
                icon: _running
                    ? const SizedBox(
                        width: 16, height: 16, child: CircularProgressIndicator(strokeWidth: 2))
                    : const Icon(Icons.travel_explore),
                label: Text(_running ? '识别中 $_done/$_total' : '开始识别'),
              ),
              const SizedBox(width: 8),
              if (_running)
                OutlinedButton.icon(
                  onPressed: () => setState(() => _cancel = true),
                  icon: const Icon(Icons.stop, size: 18),
                  label: const Text('停止'),
                ),
              const Spacer(),
              if (_rows.isNotEmpty)
                Text('自动写入 $_autoWritten 本 · 共 ${_rows.length} 本',
                    style: theme.textTheme.bodySmall),
            ],
          ),
          if (_running) ...[
            const SizedBox(height: 8),
            LinearProgressIndicator(value: _total == 0 ? null : _done / _total),
          ],
          if (_rows.isNotEmpty) ...[
            const SizedBox(height: 12),
            for (final r in _rows) _rowTile(r, theme),
          ],
        ],
      ),
    );
  }

  Widget _rowTile(_BatchRow r, ThemeData theme) {
    final scheme = theme.colorScheme;
    final color = r.matchedUnique
        ? scheme.primary
        : (r.error != null ? scheme.error : scheme.outline);
    final plan = r.plan;
    final tags = (plan?['tags'] as List?) ?? const [];
    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: color.withValues(alpha: 0.35)),
        color: scheme.surfaceContainerHighest.withValues(alpha: 0.3),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(r.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600)),
              ),
              const SizedBox(width: 8),
              Text(r.statusLabel, style: TextStyle(fontSize: 12, color: color)),
            ],
          ),
          if (r.error != null)
            Text(r.error!, style: theme.textTheme.bodySmall?.copyWith(color: scheme.error)),
          if (plan?['title_jpn'] != null && (plan!['title_jpn'] as String).isNotEmpty)
            Text('→ ${plan['title_jpn']}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: theme.textTheme.bodySmall),
          if (tags.isNotEmpty) ...[
            const SizedBox(height: 4),
            Wrap(
              spacing: 6,
              runSpacing: 4,
              children: tags
                  .take(14)
                  .map((t) => TagBox(name: (t as Map)['name'] as String))
                  .toList(),
            ),
          ],
        ],
      ),
    );
  }
}
