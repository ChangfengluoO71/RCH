import 'dart:convert';

import 'package:app/src/rust/api/scraper.dart' as scraperapi;
import 'package:app/store/automation_coordinator.dart';
import 'package:flutter/material.dart';

/// Review surface for the catalog-only scraper. It shows filename/path
/// evidence and the result of the catalog proposal review policy.
class ScrapePanel extends StatefulWidget {
  const ScrapePanel({super.key});

  @override
  State<ScrapePanel> createState() => _ScrapePanelState();
}

class _ScrapePanelState extends State<ScrapePanel> {
  List<scraperapi.ScrapeProposalDto> _proposals = [];
  scraperapi.ScrapeRunDto? _lastRun;
  String _state = '';
  bool _loading = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _reload();
  }

  Future<void> _reload() async {
    try {
      final proposals = await scraperapi.dbLoadScrapeProposals(
        limit: 100,
        state: _state,
      );
      if (!mounted) return;
      setState(() {
        _proposals = proposals;
        _error = null;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = '$error');
    }
  }

  Future<void> _runScrape() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    final run = await AutomationCoordinator.instance.runScrapeNow();
    await _reload();
    if (!mounted) return;
    setState(() {
      _lastRun = run;
      _loading = false;
      if (run == null) _error = AutomationCoordinator.instance.lastStatus;
    });
  }

  String _authors(String json) {
    try {
      final value = jsonDecode(json);
      if (value is List) return value.whereType<String>().join('、');
    } catch (_) {}
    return '';
  }

  List<String> _jsonList(String json) {
    try {
      final value = jsonDecode(json);
      if (value is List) return value.whereType<String>().toList();
    } catch (_) {}
    return const <String>[];
  }

  bool _jsonArrayHasItems(String json) {
    try {
      final value = jsonDecode(json);
      return value is List && value.isNotEmpty;
    } catch (_) {
      return true;
    }
  }

  String _evidence(String json) {
    try {
      final value = jsonDecode(json);
      if (value is List) {
        return value
            .whereType<Map>()
            .map(
              (item) => '${item['role']}: ${item['value']} (${item['rule']})',
            )
            .join(' · ');
      }
    } catch (_) {}
    return '';
  }

  Map<String, dynamic> _semantic(String json) {
    try {
      final value = jsonDecode(json);
      if (value is Map) return Map<String, dynamic>.from(value);
    } catch (_) {}
    return const <String, dynamic>{};
  }

  String _semanticDetails(String json) {
    final semantic = _semantic(json);
    final identity = semantic['identity'] is Map
        ? Map<String, dynamic>.from(semantic['identity'] as Map)
        : semantic;
    final publication = semantic['publication'] is Map
        ? Map<String, dynamic>.from(semantic['publication'] as Map)
        : semantic;
    final sequence = semantic['sequence'] is Map
        ? Map<String, dynamic>.from(semantic['sequence'] as Map)
        : semantic;
    final release = semantic['release'] is Map
        ? Map<String, dynamic>.from(semantic['release'] as Map)
        : semantic;
    final parts = <String>[];
    final creators = semantic['creators'] is List
        ? (semantic['creators'] as List)
              .whereType<Map>()
              .map((item) {
                final name = item['name'];
                final role = item['role'];
                return name is String && name.isNotEmpty
                    ? '$role: $name'
                    : null;
              })
              .whereType<String>()
              .join(', ')
        : '';
    if (creators.isNotEmpty) parts.add(creators);
    final sourceSeries = semantic['source_series'];
    if (sourceSeries is List && sourceSeries.isNotEmpty) {
      parts.add('原作系列: ${sourceSeries.join(', ')}');
    }
    final event = publication['release_event'];
    if (event is String && event.isNotEmpty) parts.add('活动: $event');
    final sequenceKind = sequence['sequence_kind'];
    if (sequenceKind is String && sequenceKind.isNotEmpty) {
      final chapter = sequence['chapter'];
      final chapterTitle = sequence['chapter_title'];
      final relation = sequence['chapter_relation'];
      final sortKey = sequence['sort_key'];
      final sequenceText = <String>[sequenceKind];
      if (chapter is String && chapter.isNotEmpty) sequenceText.add(chapter);
      if (chapterTitle is String && chapterTitle.isNotEmpty) {
        sequenceText.add(chapterTitle);
      }
      if (relation is String && relation.isNotEmpty) sequenceText.add(relation);
      if (sortKey is String && sortKey.isNotEmpty) {
        sequenceText.add('sort=$sortKey');
      } else if (sortKey is Map) {
        final major = sortKey['major'];
        final minor = sortKey['minor'];
        final relationRank = sortKey['relation_rank'];
        sequenceText.add(
          'sort=$major${minor == null ? '' : '.$minor'}@$relationRank',
        );
      }
      parts.add(sequenceText.join(' '));
    }
    final edition = publication['edition'];
    if (edition is String && edition.isNotEmpty) parts.add('版本: $edition');
    final language = release['resource_language'];
    if (language is String && language.isNotEmpty) parts.add('语言: $language');
    final sourceMedium = release['source_medium'];
    if (sourceMedium is String && sourceMedium.isNotEmpty) {
      parts.add('来源: $sourceMedium');
    }
    final tags = release['resource_tags'];
    if (tags is List && tags.isNotEmpty) parts.add('资源: ${tags.join(', ')}');
    final externalIds = identity['external_id_candidates'];
    if (externalIds is List && externalIds.isNotEmpty) {
      final ids = externalIds
          .whereType<Map>()
          .map((item) => item['raw'])
          .whereType<String>()
          .join(', ');
      if (ids.isNotEmpty) parts.add('外部编号: $ids');
    }
    return parts.join(' · ');
  }

  Color _stateColor(String state) {
    return switch (state) {
      'ready' => Colors.green,
      'ambiguous' => Colors.orange,
      'partial' => Colors.blueGrey,
      _ => Colors.grey,
    };
  }

  String _stateLabel(String state) {
    return switch (state) {
      'ready' => '可确认',
      'ambiguous' => '需复核',
      'partial' => '信息不完整',
      _ => '未匹配',
    };
  }

  @override
  Widget build(BuildContext context) {
    final run = _lastRun;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                const Expanded(
                  child: Text(
                    '智能刮削（Catalog-only）',
                    style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
                  ),
                ),
                FilledButton.icon(
                  onPressed: _loading ? null : _runScrape,
                  icon: _loading
                      ? const SizedBox(
                          width: 16,
                          height: 16,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.auto_awesome, size: 18),
                  label: const Text('仅生成刮削 proposal'),
                ),
              ],
            ),
            const SizedBox(height: 4),
            const Text(
              '只读取本地 SQLite 中的文件名与上级目录；远程未缓存漫画不会触发 Range、stat 或下载。当前生产自动写入已暂停。',
              style: TextStyle(fontSize: 12, color: Colors.grey),
            ),
            const Text(
              'Ready、Partial、Ambiguous 等结果均先保留为 proposal，待 asset identity 与 115 目录归一化验收后再开放自动写入。',
              style: TextStyle(fontSize: 12, color: Colors.blueGrey),
            ),
            if (run != null) ...[
              const SizedBox(height: 8),
              Text(
                '最近任务：${run.status} · ${run.processed}/${run.total} · 可确认 ${run.ready} · 需复核 ${run.ambiguous} · 不完整 ${run.partial}',
                style: const TextStyle(fontSize: 12),
              ),
              Text(
                '物理资产 ${run.inputAssets}/${run.uniqueAssets} · proposal ${run.proposalsWritten} · 冲突 ${run.assetCollisionCount} · 记账 ${run.accountingStatus}',
                style: const TextStyle(fontSize: 12, color: Colors.blueGrey),
              ),
            ],
            if (_error != null) ...[
              const SizedBox(height: 6),
              Text(
                _error!,
                style: const TextStyle(color: Colors.red, fontSize: 12),
              ),
            ],
            const SizedBox(height: 8),
            Row(
              children: [
                const Text('显示：', style: TextStyle(fontSize: 12)),
                DropdownButton<String>(
                  value: _state,
                  items: const [
                    DropdownMenuItem(value: '', child: Text('全部')),
                    DropdownMenuItem(value: 'ready', child: Text('可确认')),
                    DropdownMenuItem(value: 'ambiguous', child: Text('需复核')),
                    DropdownMenuItem(value: 'partial', child: Text('信息不完整')),
                    DropdownMenuItem(value: 'unmatched', child: Text('未匹配')),
                  ],
                  onChanged: (value) {
                    if (value == null) return;
                    setState(() => _state = value);
                    _reload();
                  },
                ),
                const Spacer(),
                Text(
                  '${_proposals.length} 条',
                  style: const TextStyle(fontSize: 12, color: Colors.grey),
                ),
              ],
            ),
            const Divider(height: 12),
            if (_proposals.isEmpty)
              const Text(
                '暂无刮削结果，请先运行刮削或确认本地/远程目录已经进入离线索引。',
                style: TextStyle(fontSize: 12, color: Colors.grey),
              )
            else
              ..._proposals.map((proposal) {
                final authors = _authors(proposal.authorsJson);
                final evidence = _evidence(proposal.evidenceJson);
                final semanticDetails = _semanticDetails(proposal.semanticJson);
                final title = proposal.title?.trim().isNotEmpty == true
                    ? proposal.title!
                    : '（未识别作品名）';
                final materializationLabel =
                    proposal.state != 'ready' ||
                        _jsonArrayHasItems(proposal.conflictsJson)
                    ? 'review required'
                    : switch (proposal.materializationStatus) {
                        'applied' => 'auto-applied',
                        'skipped' => 'already applied / manual fields kept',
                        'stale' => 'stale input — review required',
                        'review-required' => 'review required',
                        _ => 'proposal pending review',
                      };
                final appliedFields = _jsonList(
                  proposal.materializationAppliedFieldsJson,
                );
                final addedTags = _jsonList(
                  proposal.materializationAddedTagsJson,
                );
                final skippedFields = _jsonList(
                  proposal.materializationSkippedFieldsJson,
                );
                final details = <String>[
                  if (authors.isNotEmpty) '作者：$authors',
                  if (proposal.provider?.isNotEmpty == true)
                    '提供者：${proposal.provider}',
                  if (proposal.volume?.isNotEmpty == true)
                    '卷：${proposal.volume}',
                  if (proposal.chapter?.isNotEmpty == true)
                    '章：${proposal.chapter}',
                ].join(' · ');
                return Padding(
                  padding: const EdgeInsets.only(bottom: 8),
                  child: Container(
                    width: double.infinity,
                    padding: const EdgeInsets.all(8),
                    decoration: BoxDecoration(
                      border: Border(
                        left: BorderSide(
                          color: _stateColor(proposal.state),
                          width: 3,
                        ),
                      ),
                      color: Theme.of(context)
                          .colorScheme
                          .surfaceContainerHighest
                          .withValues(alpha: 0.35),
                    ),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '$title  ·  ${_stateLabel(proposal.state)}',
                          style: const TextStyle(fontWeight: FontWeight.w600),
                        ),
                        if (details.isNotEmpty)
                          Text(details, style: const TextStyle(fontSize: 12)),
                        if (semanticDetails.isNotEmpty)
                          Text(
                            semanticDetails,
                            maxLines: 3,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(fontSize: 12),
                          ),
                        Text(
                          materializationLabel,
                          style: TextStyle(
                            fontSize: 11,
                            color: proposal.materializationStatus == 'applied'
                                ? Colors.green
                                : Colors.orange,
                          ),
                        ),
                        if (proposal.materializationError.isNotEmpty)
                          Text(
                            proposal.materializationError,
                            style: const TextStyle(
                              fontSize: 11,
                              color: Colors.red,
                            ),
                          ),
                        if (appliedFields.isNotEmpty ||
                            addedTags.isNotEmpty ||
                            skippedFields.isNotEmpty)
                          Text(
                            [
                              if (appliedFields.isNotEmpty)
                                'fields: ${appliedFields.join(', ')}',
                              if (addedTags.isNotEmpty)
                                'tags: ${addedTags.join(', ')}',
                              if (skippedFields.isNotEmpty)
                                'kept: ${skippedFields.join(', ')}',
                            ].join(' · '),
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(
                              fontSize: 11,
                              color: Colors.blueGrey,
                            ),
                          ),
                        if (evidence.isNotEmpty)
                          Text(
                            '证据：$evidence',
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(
                              fontSize: 11,
                              color: Colors.blueGrey,
                            ),
                          ),
                        Text(
                          'revision: ${proposal.inputRevision}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                            fontSize: 10,
                            color: Colors.grey,
                          ),
                        ),
                        Text(
                          proposal.filename,
                          style: const TextStyle(
                            fontSize: 11,
                            color: Colors.grey,
                          ),
                        ),
                        Text(
                          proposal.path,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                            fontSize: 11,
                            color: Colors.grey,
                          ),
                        ),
                      ],
                    ),
                  ),
                );
              }),
          ],
        ),
      ),
    );
  }
}
