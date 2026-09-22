import 'dart:async';
import 'dart:io';

import 'package:app/store/eh_subscription_store.dart';
import 'package:app/ui/common.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';

/// 设置页「EH 订阅」面板（可选插件）。
///
/// 边界：只写用户指定的保存目录（`.torrent` + `manifest.json`），
/// 不触碰书源、目录库与阅读数据；必须手动点击才会运行。
class EhSubscriptionPanel extends StatefulWidget {
  const EhSubscriptionPanel({super.key, this.storeOverride});

  /// 仅测试使用：注入预置 store（避免触碰 FFI / 磁盘）。
  final EhSubscriptionStore? storeOverride;

  @override
  State<EhSubscriptionPanel> createState() => _EhSubscriptionPanelState();
}

class _EhSubscriptionPanelState extends State<EhSubscriptionPanel> {
  final _rulesKey = GlobalKey<_RulesEditorState>();

  EhSubscriptionStore get store => widget.storeOverride ?? EhSubscriptionStore.instance;

  @override
  void initState() {
    super.initState();
    if (widget.storeOverride != null) return; // 测试注入：不初始化
    final s = EhSubscriptionStore.instance;
    if (s.progress.stage == 'idle' && s.loadError == null) {
      // 首次挂载时初始化（读规则 + 读清单）
      WidgetsBinding.instance.addPostFrameCallback((_) => s.init());
    }
  }

  Future<void> _pickDir() async {
    final store = EhSubscriptionStore.instance;
    final picked = await getDirectoryPath(
      initialDirectory: store.hasOutDir ? store.outDir : null,
      confirmButtonText: '选择此目录',
    );
    if (picked == null) return;
    _rulesKey.currentState?.setOutDir(picked);
    store.patch('out_dir', picked);
    await store.saveRules();
    await store.refreshManifest();
  }

  Future<void> _run() async {
    final editor = _rulesKey.currentState;
    editor?.flush();
    final store = EhSubscriptionStore.instance;
    if (!store.hasOutDir) {
      _snack('请先选择保存目录', error: true);
      return;
    }
    await store.saveRules();
    await store.run();
    if (!mounted) return;
    final p = store.progress;
    _snack(p.error ?? (p.message.isEmpty ? '扫描结束' : p.message), error: p.error != null);
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

  Future<bool> _confirm(String title, String body) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (c) => AlertDialog(
        title: Text(title),
        content: Text(body),
        actions: [
          TextButton(onPressed: () => Navigator.of(c).pop(false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.of(c).pop(true), child: const Text('确定')),
        ],
      ),
    );
    return ok ?? false;
  }

  Future<void> _openOutDir() async {
    final dir = EhSubscriptionStore.instance.outDir;
    if (dir.isEmpty) return;
    try {
      await Process.run('explorer', [dir.replaceAll('/', r'\')]);
    } catch (_) {
      _snack('无法打开目录：$dir');
    }
  }

  @override
  Widget build(BuildContext context) {
    final store = this.store;
    final theme = Theme.of(context);
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        if (store.loadError != null) {
          return _Section(
            title: 'EH 订阅',
            subtitle: '加载失败：${store.loadError}',
            children: [
              FilledButton.icon(
                onPressed: () => store.init(),
                icon: const Icon(Icons.refresh),
                label: const Text('重试'),
              ),
            ],
          );
        }
        if (!store.rulesLoaded) {
          return const _Section(
            title: 'EH 订阅',
            subtitle: '正在加载规则…',
            children: [LinearProgressIndicator()],
          );
        }
        return _Section(
          title: 'EH 订阅',
          subtitle: '按规则筛选 E-Hentai 画廊种子并保存到指定文件夹。可选插件：只写该文件夹，'
              '不改动书源、目录与阅读数据；需手动点击运行。',
          trailing: _StatusPill(progress: store.progress),
          children: [
            Card(
              margin: EdgeInsets.zero,
              child: Padding(
                padding: const EdgeInsets.all(16),
                child: _RulesEditor(key: _rulesKey, store: store, onPickDir: _pickDir),
              ),
            ),
            const SizedBox(height: 12),
            _RunBar(store: store, onRun: _run, onOpenDir: _openOutDir),
            const SizedBox(height: 12),
            _ProgressBlock(progress: store.progress),
            if (store.manifest.isNotEmpty) ...[
              const SizedBox(height: 16),
              _ManifestBlock(store: store, onOpenDir: _openOutDir),
            ],
            const SizedBox(height: 8),
            Align(
              alignment: Alignment.centerLeft,
              child: TextButton.icon(
                onPressed: () async {
                  if (!await _confirm(
                    '恢复默认规则',
                    '将把搜索语法、评分、标记与分档阈值恢复为默认值（已保存的种子与清单不受影响）。',
                  )) {
                    return;
                  }
                  await store.resetToDefaults();
                  _rulesKey.currentState?.reloadFromStore();
                  _snack('已恢复默认规则');
                },
                icon: const Icon(Icons.restart_alt),
                label: const Text('恢复默认规则'),
              ),
            ),
            Align(
              alignment: Alignment.centerLeft,
              child: Text(
                r'标签语法示例：language:"chinese"$ · other:"full color"$ · uncensored',
                style: theme.textTheme.bodySmall,
              ),
            ),
          ],
        );
      },
    );
  }
}

/// 面板骨架：标题 + 说明 + 内容（与设置页其它面板一致）。
class _Section extends StatelessWidget {
  const _Section({
    required this.title,
    required this.subtitle,
    required this.children,
    this.trailing,
  });

  final String title;
  final String subtitle;
  final List<Widget> children;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            const Text('EH 订阅', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)),
            const Spacer(),
            ?trailing,
          ],
        ),
        const SizedBox(height: 4),
        Text(subtitle, style: theme.textTheme.bodySmall),
        const SizedBox(height: 12),
        ...children,
      ],
    );
  }
}

/// 顶部状态徽标：跑动中/上次结果。
class _StatusPill extends StatelessWidget {
  const _StatusPill({required this.progress});

  final EhProgress progress;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final (label, color) = switch (progress.stage) {
      'failed' => ('失败', scheme.error),
      'done' => ('就绪', scheme.primary),
      'idle' => ('未运行', scheme.outline),
      _ => (progress.stageLabel, scheme.tertiary),
    };
    final running = progress.running;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: color.withValues(alpha: 0.5)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (running) ...[
            SizedBox(
              width: 12,
              height: 12,
              child: CircularProgressIndicator(strokeWidth: 2, color: color),
            ),
            const SizedBox(width: 6),
          ] else ...[
            Icon(Icons.circle, size: 8, color: color),
            const SizedBox(width: 6),
          ],
          Text(label, style: TextStyle(fontSize: 12, color: color, fontWeight: FontWeight.w600)),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// 规则编辑
// ---------------------------------------------------------------------------

class _RulesEditor extends StatefulWidget {
  const _RulesEditor({super.key, required this.store, required this.onPickDir});

  final EhSubscriptionStore store;
  final Future<void> Function() onPickDir;

  @override
  State<_RulesEditor> createState() => _RulesEditorState();
}

class _TierRow {
  _TierRow(double years, int downloads)
    : yearsCtrl = TextEditingController(text: _fmtNum(years)),
      downloadsCtrl = TextEditingController(text: '$downloads');

  final TextEditingController yearsCtrl;
  final TextEditingController downloadsCtrl;

  void dispose() {
    yearsCtrl.dispose();
    downloadsCtrl.dispose();
  }
}

String _fmtNum(double v) => v == v.roundToDouble() ? '${v.toInt()}' : '$v';

class _RulesEditorState extends State<_RulesEditor> {
  late final TextEditingController _search;
  late final TextEditingController _markers;
  late final TextEditingController _exclude;
  late final TextEditingController _interval;
  late final TextEditingController _pagesCtrl;
  late final TextEditingController _hostCtrl;
  List<_TierRow> _tiers = [];

  double _minRating = 4;
  int _pages = 2;
  Timer? _debounce;

  EhSubscriptionStore get store => widget.store;

  @override
  void initState() {
    super.initState();
    reloadFromStore();
  }

  void reloadFromStore() {
    final s = store;
    _search = TextEditingController(text: s.search);
    _markers = TextEditingController(text: s.titleMarkers);
    _exclude = TextEditingController(text: s.excludeMarkers.join(', '));
    _interval = TextEditingController(text: '${s.intervalSecs}');
    _pagesCtrl = TextEditingController(text: '${s.pages}');
    _hostCtrl = TextEditingController(text: s.host.isEmpty ? 'e-hentai.org' : s.host);
    _minRating = s.minRating;
    _pages = s.pages;
    for (final t in _tiers) {
      t.dispose();
    }
    _tiers = s.ageTiers
        .map(
          (t) => _TierRow(
            (t['min_age_years'] as num?)?.toDouble() ?? 0,
            (t['min_downloads'] as num?)?.toInt() ?? 0,
          ),
        )
        .toList();
    setState(() {});
  }

  /// 目录选择后由外部写入（控制器不在本编辑器内）。
  void setOutDir(String dir) {
    store.patch('out_dir', dir);
    setState(() {});
  }

  /// 把编辑器内容统一写回 store（运行/保存前调用）。
  void flush() {
    store.patch('search', _search.text.trim());
    store.patch('title_markers', _markers.text.trim());
    store.patch(
      'exclude_markers',
      _exclude.text
          .split(RegExp(r'[,，]'))
          .map((e) => e.trim())
          .where((e) => e.isNotEmpty)
          .toList(),
    );
    store.patch('min_rating', _minRating);
    final custom = int.tryParse(_pagesCtrl.text.trim());
    if (custom != null && custom > 0) {
      _pages = custom.clamp(1, 500);
    }
    store.patch('pages', _pages);
    final hostText = _hostCtrl.text.trim();
    store.patch('host', hostText.isEmpty ? 'e-hentai.org' : hostText);
    store.patch('request_interval_secs', double.tryParse(_interval.text.trim()) ?? 2.5);
    store.patch('age_tiers', _collectTiers());
  }

  List<Map<String, dynamic>> _collectTiers() {
    final rows = _tiers
        .map(
          (t) => {
            'min_age_years': double.tryParse(t.yearsCtrl.text.trim()) ?? 0,
            'min_downloads': int.tryParse(t.downloadsCtrl.text.trim()) ?? 0,
          },
        )
        .toList();
    rows.sort(
      (a, b) => (b['min_age_years'] as double).compareTo(a['min_age_years'] as double),
    );
    return rows;
  }

  /// 输入防抖写回：用户停手 700ms 后更新 store（不触发运行）。
  void _debouncedFlush() {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 700), flush);
  }

  @override
  void dispose() {
    _debounce?.cancel();
    _search.dispose();
    _markers.dispose();
    _exclude.dispose();
    _interval.dispose();
    _pagesCtrl.dispose();
    _hostCtrl.dispose();
    for (final t in _tiers) {
      t.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = widget.store;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _FieldLabel('搜索语法', 'EH 服务端唯一能筛的维度：语言与标签（评分/有无种子只能本地筛）'),
        TextField(
          controller: _search,
          onChanged: (_) => _debouncedFlush(),
          decoration: const InputDecoration(
            isDense: true,
            border: OutlineInputBorder(),
            hintText: r'language:chinese$ uncensored',
            prefixIcon: Icon(Icons.search),
          ),
        ),
        const SizedBox(height: 16),
        _FieldLabel('评分下限：${_minRating.toStringAsFixed(1)}', '0 票的画廊评分为 0.00，会被自动排除'),
        Slider(
          value: _minRating.clamp(0, 5),
          min: 0,
          max: 5,
          divisions: 50,
          label: _minRating.toStringAsFixed(1),
          onChanged: (v) => setState(() => _minRating = v),
          onChangeEnd: (_) => flush(),
        ),
        const SizedBox(height: 8),
        _FieldLabel('标题标记（白名单）', '用 | 分隔；实测 [Digital]/[DL版] 出现在标题而非标签里。填 any 表示不筛'),
        TextField(
          controller: _markers,
          onChanged: (_) => _debouncedFlush(),
          decoration: const InputDecoration(
            isDense: true,
            border: OutlineInputBorder(),
            hintText: 'Digital|DL版|DL',
          ),
        ),
        const SizedBox(height: 12),
        _FieldLabel('排除标记', '命中即排除，用逗号分隔'),
        TextField(
          controller: _exclude,
          onChanged: (_) => _debouncedFlush(),
          decoration: const InputDecoration(
            isDense: true,
            border: OutlineInputBorder(),
            hintText: 'AI Generated',
          ),
        ),
        const SizedBox(height: 20),
        Row(
          children: [
            _FieldLabel('分时间段下载数要求', '画龄越久要求越高；新发不设门槛，靠重复扫描累积达标'),
            const Spacer(),
            TextButton.icon(
              onPressed: () {
                setState(() {
                  _tiers.insert(0, _TierRow(5, 800));
                });
                flush();
              },
              icon: const Icon(Icons.add, size: 18),
              label: const Text('添加档位'),
            ),
          ],
        ),
        const SizedBox(height: 4),
        Container(
          decoration: BoxDecoration(
            border: Border.all(color: theme.dividerColor),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Column(
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(12, 8, 12, 0),
                child: Row(
                  children: [
                    Expanded(flex: 4, child: Text('画龄 ≥（年）', style: theme.textTheme.bodySmall)),
                    Expanded(flex: 4, child: Text('下载数 ≥', style: theme.textTheme.bodySmall)),
                    const SizedBox(width: 40),
                  ],
                ),
              ),
              for (var i = 0; i < _tiers.length; i++)
                Padding(
                  padding: const EdgeInsets.fromLTRB(12, 4, 4, 4),
                  child: Row(
                    children: [
                      Expanded(
                        flex: 4,
                        child: TextField(
                          controller: _tiers[i].yearsCtrl,
                          onChanged: (_) => _debouncedFlush(),
                          keyboardType: const TextInputType.numberWithOptions(decimal: true),
                          decoration: const InputDecoration(isDense: true, border: OutlineInputBorder()),
                        ),
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        flex: 4,
                        child: TextField(
                          controller: _tiers[i].downloadsCtrl,
                          onChanged: (_) => _debouncedFlush(),
                          keyboardType: TextInputType.number,
                          decoration: const InputDecoration(isDense: true, border: OutlineInputBorder()),
                        ),
                      ),
                      IconButton(
                        tooltip: '删除该档位',
                        onPressed: _tiers.length <= 1
                            ? null
                            : () {
                                setState(() {
                                  _tiers.removeAt(i).dispose();
                                });
                                flush();
                              },
                        icon: const Icon(Icons.remove_circle_outline, size: 20),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
        const SizedBox(height: 20),
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Expanded(
              flex: 3,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _FieldLabel('保存目录', '种子与 manifest.json 落在这里'),
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          store.hasOutDir ? store.outDir : '（未选择）',
                          style: theme.textTheme.bodyMedium?.copyWith(
                            color: store.hasOutDir ? null : theme.colorScheme.error,
                          ),
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                      const SizedBox(width: 8),
                      OutlinedButton.icon(
                        onPressed: widget.onPickDir,
                        icon: const Icon(Icons.folder_open, size: 18),
                        label: const Text('选择'),
                      ),
                    ],
                  ),
                ],
              ),
            ),
            const SizedBox(width: 16),
            Expanded(
              flex: 3,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _FieldLabel('每轮页数', '每页 25 条（约 40~60 秒/页）；上限 500。覆盖越广耗时越长'),
                  SizedBox(
                    width: 140,
                    child: TextField(
                      controller: _pagesCtrl,
                      keyboardType: TextInputType.number,
                      decoration: const InputDecoration(
                        isDense: true,
                        border: OutlineInputBorder(),
                        suffixText: '页',
                      ),
                      onChanged: (_) => _debouncedFlush(),
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 16),
            Expanded(
              flex: 2,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _FieldLabel('请求间隔（秒）', '过小会触发 EH 限流，建议 ≥2'),
                  TextField(
                    controller: _interval,
                    onChanged: (_) => _debouncedFlush(),
                    keyboardType: const TextInputType.numberWithOptions(decimal: true),
                    decoration: const InputDecoration(isDense: true, border: OutlineInputBorder()),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 16),
            Expanded(
              flex: 3,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _FieldLabel('主站域名', '默认 e-hentai.org。国内需代理；也可填可访问的镜像域名'),
                  Row(
                    children: [
                      Expanded(
                        child: TextField(
                          controller: _hostCtrl,
                          onChanged: (_) => _debouncedFlush(),
                          decoration: const InputDecoration(isDense: true, border: OutlineInputBorder()),
                        ),
                      ),
                      const SizedBox(width: 6),
                      OutlinedButton(
                        onPressed: widget.store.probing ? null : _checkConnectivity,
                        child: widget.store.probing
                            ? const SizedBox(width: 14, height: 14, child: CircularProgressIndicator(strokeWidth: 2))
                            : const Text('检查'),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ],
        ),
        if (widget.store.probe != null) ...[
          const SizedBox(height: 12),
          _ProbeBlock(probe: widget.store.probe!),
        ],
      ],
    );
  }

  Future<void> _checkConnectivity() async {
    flush();
    final p = await widget.store.runProbe();
    if (!mounted || p == null) return;
    final ok = p.hostOk && p.trackerOk;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text('主站 ${p.host}: ${p.hostOk ? "可达" : "不可达（${p.hostDetail}）"} ｜ '
            '${p.tracker}: ${p.trackerOk ? "可达" : "不可达（${p.trackerDetail}）"}'),
        backgroundColor: ok ? null : Theme.of(context).colorScheme.error,
        duration: const Duration(seconds: 6),
      ),
    );
  }
}

/// 连通性预检结果块：把"哪一段不通"讲清楚。
class _ProbeBlock extends StatelessWidget {
  const _ProbeBlock({required this.probe});

  final EhProbe probe;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    Widget row(bool ok, String label, String detail) {
      final color = ok ? scheme.primary : scheme.error;
      return Padding(
        padding: const EdgeInsets.only(top: 4),
        child: Row(
          children: [
            Icon(ok ? Icons.check_circle_outline : Icons.error_outline, size: 14, color: color),
            const SizedBox(width: 6),
            Text('$label：${ok ? "可达" : "不可达"}', style: TextStyle(fontSize: 12, color: color)),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                detail,
                style: Theme.of(context).textTheme.bodySmall?.copyWith(fontSize: 11),
                overflow: TextOverflow.ellipsis,
              ),
            ),
          ],
        ),
      );
    }

    return Container(
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 10),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: scheme.outlineVariant),
        color: scheme.surfaceContainerHighest.withValues(alpha: 0.3),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('连通性检查', style: TextStyle(fontSize: 12, fontWeight: FontWeight.w600, color: scheme.outline)),
          row(probe.hostOk, probe.host, probe.hostDetail),
          row(probe.trackerOk, probe.tracker, probe.trackerDetail),
          if (!probe.hostOk)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                '主站不可达时：① 用系统代理访问；② 或把「主站域名」改成你能访问的镜像域名再点「检查」。',
                style: Theme.of(context).textTheme.bodySmall?.copyWith(fontSize: 11),
              ),
            ),
        ],
      ),
    );
  }
}

class _FieldLabel extends StatelessWidget {
  const _FieldLabel(this.title, this.hint);

  final String title;
  final String hint;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(title, style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 13)),
          Text(hint, style: theme.textTheme.bodySmall?.copyWith(fontSize: 11)),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// 运行条 + 进度 + 已保存清单
// ---------------------------------------------------------------------------

class _RunBar extends StatelessWidget {
  const _RunBar({required this.store, required this.onRun, required this.onOpenDir});

  final EhSubscriptionStore store;
  final Future<void> Function() onRun;
  final Future<void> Function() onOpenDir;

  @override
  Widget build(BuildContext context) {
    final running = store.progress.running || store.busy;
    return Row(
      children: [
        FilledButton.icon(
          onPressed: running ? null : () => onRun(),
          icon: running
              ? const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : const Icon(Icons.download),
          label: Text(running ? '扫描中…' : '开始扫描'),
        ),
        const SizedBox(width: 8),
        if (running)
          OutlinedButton.icon(
            onPressed: () => store.cancel(),
            icon: const Icon(Icons.stop, size: 18),
            label: const Text('停止'),
          ),
        if (!running)
          OutlinedButton.icon(
            onPressed: store.hasOutDir ? () => onOpenDir() : null,
            icon: const Icon(Icons.folder_open, size: 18),
            label: const Text('打开目录'),
          ),
        const Spacer(),
        Text(
          '已保存 ${store.manifest.length} 个',
          style: Theme.of(context).textTheme.bodySmall,
        ),
      ],
    );
  }
}

class _ProgressBlock extends StatelessWidget {
  const _ProgressBlock({required this.progress});

  final EhProgress progress;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = progress;
    if (p.stage == 'idle') {
      return Text('尚未运行。点击「开始扫描」按当前规则抓取一轮。', style: theme.textTheme.bodySmall);
    }
    final total = p.pages <= 0 ? 1 : p.pages;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (p.running) ...[
          LinearProgressIndicator(
            value: p.stage == 'searching' ? (p.page / total).clamp(0, 1) : null,
          ),
          const SizedBox(height: 8),
        ],
        Wrap(
          spacing: 8,
          runSpacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            _Metric(icon: Icons.travel_explore, label: '候选', value: '${p.candidates}'),
            _Metric(icon: Icons.download_done, label: '本轮新增', value: '${p.saved}', highlight: true),
            _Metric(icon: Icons.star_border, label: '评分不足', value: '${p.noRating}'),
            _Metric(icon: Icons.label_off_outlined, label: '标记排除', value: '${p.noMarker}'),
            _Metric(icon: Icons.cloud_off, label: '无种子', value: '${p.noTorrent}'),
            _Metric(icon: Icons.trending_down, label: '下载数不达标', value: '${p.noDownloads}'),
            _Metric(icon: Icons.help_outline, label: '映射未确认', value: '${p.unmapped}'),
          ],
        ),
        const SizedBox(height: 8),
        if (p.error != null)
          Text('失败：${p.error}', style: TextStyle(color: theme.colorScheme.error, fontSize: 12))
        else if (p.message.isNotEmpty)
          Text(p.message, style: theme.textTheme.bodySmall),
      ],
    );
  }
}

class _Metric extends StatelessWidget {
  const _Metric({
    required this.icon,
    required this.label,
    required this.value,
    this.highlight = false,
  });

  final IconData icon;
  final String label;
  final String value;
  final bool highlight;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final color = highlight && value != '0' ? scheme.primary : scheme.outline;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.10),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: color.withValues(alpha: 0.35)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 14, color: color),
          const SizedBox(width: 6),
          Text('$label $value', style: TextStyle(fontSize: 12, color: color)),
        ],
      ),
    );
  }
}

class _ManifestBlock extends StatefulWidget {
  const _ManifestBlock({required this.store, required this.onOpenDir});

  final EhSubscriptionStore store;
  final Future<void> Function() onOpenDir;

  @override
  State<_ManifestBlock> createState() => _ManifestBlockState();
}

class _ManifestBlockState extends State<_ManifestBlock> {
  bool _showAll = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final items = widget.store.manifest;
    final shown = _showAll ? items : items.take(5).toList();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            const Text('已保存种子', style: TextStyle(fontWeight: FontWeight.w600, fontSize: 13)),
            const SizedBox(width: 8),
            Text('共 ${items.length} 个', style: theme.textTheme.bodySmall),
            const Spacer(),
            TextButton.icon(
              onPressed: () => widget.onOpenDir(),
              icon: const Icon(Icons.folder_open, size: 16),
              label: const Text('打开目录'),
            ),
            if (items.length > 5)
              TextButton(
                onPressed: () => setState(() => _showAll = !_showAll),
                child: Text(_showAll ? '收起' : '展开全部'),
              ),
          ],
        ),
        const SizedBox(height: 4),
        for (final item in shown) _ManifestTile(item: item),
        if (!_showAll && items.length > 5)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text('仅显示最近 5 个', style: theme.textTheme.bodySmall),
          ),
      ],
    );
  }
}

class _ManifestTile extends StatelessWidget {
  const _ManifestTile({required this.item});

  final EhSavedItem item;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final size = item.filesize == null ? null : fmtSize(BigInt.from(item.filesize!));
    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest.withValues(alpha: 0.4),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(item.titleJpn, style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600)),
          const SizedBox(height: 6),
          Wrap(
            spacing: 10,
            runSpacing: 4,
            children: [
              _Chip(icon: Icons.star, text: item.rating.toStringAsFixed(2), color: scheme.tertiary),
              _Chip(
                icon: Icons.download,
                text: '${item.downloads}'
                    '${item.requiredDl > 0 ? ' / ≥${item.requiredDl}' : ''}',
                color: scheme.primary,
              ),
              _Chip(icon: Icons.event, text: item.postedUtc, color: scheme.outline),
              if (item.ageYears != null)
                _Chip(icon: Icons.hourglass_bottom, text: '${item.ageYears} 年', color: scheme.outline),
              _Chip(icon: Icons.category_outlined, text: item.category, color: scheme.outline),
              if (item.filecount != null)
                _Chip(icon: Icons.menu_book_outlined, text: '${item.filecount} 页', color: scheme.outline),
              if (size != null) _Chip(icon: Icons.sd_storage_outlined, text: size, color: scheme.outline),
              if (item.tags.isNotEmpty)
                _Chip(icon: Icons.sell_outlined, text: '${item.tags.length} 标签', color: scheme.outline),
            ],
          ),
          const SizedBox(height: 4),
          Text(
            item.file,
            style: theme.textTheme.bodySmall?.copyWith(fontSize: 11),
            overflow: TextOverflow.ellipsis,
          ),
        ],
      ),
    );
  }
}

class _Chip extends StatelessWidget {
  const _Chip({required this.icon, required this.text, required this.color});

  final IconData icon;
  final String text;
  final Color color;

  @override
  Widget build(BuildContext context) => Row(
    mainAxisSize: MainAxisSize.min,
    children: [
      Icon(icon, size: 12, color: color),
      const SizedBox(width: 4),
      Text(text, style: TextStyle(fontSize: 11, color: color)),
    ],
  );
}
