import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:app/ui/cover_editor_page.dart';
import 'package:app/ui/opener.dart';
import 'package:flutter/material.dart';

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

  @override void initState() { super.initState();
    _meta = LibraryStore.instance.metaOf(widget.source, widget.path);
    _titleCtrl = TextEditingController(text: _meta.title.isEmpty ? widget.title : _meta.title);
    _cnTitleCtrl = TextEditingController(text: _meta.chineseTitle);
    _authorCtrl = TextEditingController(text: _meta.author);
    _genreCtrl = TextEditingController(text: _meta.genre);
    _seriesCtrl = TextEditingController(text: _meta.series);
    _summaryCtrl = TextEditingController(text: _meta.summary);
    _commentCtrl = TextEditingController(text: _meta.comment);
  }

  @override void dispose() { _titleCtrl.dispose(); _cnTitleCtrl.dispose(); _authorCtrl.dispose(); _genreCtrl.dispose(); _seriesCtrl.dispose(); _summaryCtrl.dispose(); _commentCtrl.dispose(); super.dispose(); }

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

  void _addTag(String t) { t = t.trim(); if (t.isEmpty || _meta.tags.contains(t)) return; setState(() => _meta.tags.add(t)); LibraryStore.instance.updateMeta(_meta); setState(() => _tagInputKey++); }
  void _removeTag(String t) { setState(() => _meta.tags.remove(t)); LibraryStore.instance.updateMeta(_meta); }

  Widget _metaField(String label, TextEditingController ctrl, {String? hint}) => Padding(
    padding: const EdgeInsets.only(bottom: 8),
    child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Row(children: [Icon(Icons.label, size: 16, color: Colors.redAccent.shade200), const SizedBox(width: 4), Text(label, style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 13))]),
      const SizedBox(height: 4),
      TextField(controller: ctrl, decoration: InputDecoration(hintText: hint, border: const OutlineInputBorder(), isDense: true), onChanged: (_) => _saveMeta()),
    ]),
  );

  @override Widget build(BuildContext context) {
    final record = LibraryStore.instance.recordOf(widget.source, widget.path);
    return Scaffold(
      appBar: AppBar(title: const Text('漫画详情')),
      body: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Padding(padding: const EdgeInsets.all(20), child: Column(children: [
          SizedBox(width: 220, height: 310, child: ComicCover(source: widget.source, path: widget.path, force: true)),
          const SizedBox(height: 16),
          SizedBox(width: 220, child: FilledButton.icon(onPressed: () => openBook(context, widget.source, widget.path, widget.title), icon: const Icon(Icons.menu_book), label: const Text('开始阅读'))),
          const SizedBox(height: 8),
          SizedBox(width: 220, child: OutlinedButton.icon(onPressed: () async { await Navigator.of(context).push(MaterialPageRoute(builder: (_) => CoverEditorPage(source: widget.source, path: widget.path, title: widget.title))); setState(() {}); }, icon: const Icon(Icons.crop), label: const Text('自定义封面'))),
        ])),
        const VerticalDivider(width: 1),
        Expanded(child: ListView(padding: const EdgeInsets.all(20), children: [
          Text(widget.title, style: const TextStyle(fontSize: 18, fontWeight: FontWeight.bold)),
          const SizedBox(height: 8),
          if (record != null) Text('阅读进度:第 ${record.lastPage + 1} 页 · 看过 ${record.readCount} 次', style: Theme.of(context).textTheme.bodySmall),
          const SizedBox(height: 20),
          const Text('元数据标签', style: TextStyle(fontWeight: FontWeight.w600, fontSize: 14)),
          const SizedBox(height: 4),
          Text('作者/类别/系列用于管理和检索', style: Theme.of(context).textTheme.bodySmall),
          const SizedBox(height: 8),
          _metaField('标题', _titleCtrl, hint: '漫画标题'),
          _metaField('中文标题', _cnTitleCtrl, hint: '中文译名'),
          _metaField('作者', _authorCtrl, hint: '漫画作者'),
          _metaField('类别', _genreCtrl, hint: '如:同人/原创/全彩'),
          _metaField('系列', _seriesCtrl, hint: '系列名或卷号'),
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
          TextField(controller: _summaryCtrl, maxLines: 4, decoration: const InputDecoration(hintText: '这本书讲了什么…', border: OutlineInputBorder()), onChanged: (_) => _saveMeta()),
          const SizedBox(height: 20),
          const Text('感想', style: TextStyle(fontWeight: FontWeight.w600)),
          const SizedBox(height: 8),
          TextField(controller: _commentCtrl, maxLines: 4, decoration: const InputDecoration(hintText: '你的读后感…', border: OutlineInputBorder()), onChanged: (_) => _saveMeta()),
        ])),
      ]),
    );
  }
}