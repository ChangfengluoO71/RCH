import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/ui/common.dart';
import 'package:app/ui/reader_page.dart';
import 'package:flutter/material.dart';

/// WebDAV 书源页:连接服务器 → 浏览远程目录 → 打开远程漫画(流式阅读)。
class WebDavPage extends StatefulWidget {
  const WebDavPage({super.key});

  @override
  State<WebDavPage> createState() => _WebDavPageState();
}

class _WebDavPageState extends State<WebDavPage> {
  WebDavSession? _session;
  final TextEditingController _urlCtrl = TextEditingController();
  final TextEditingController _userCtrl = TextEditingController();
  final TextEditingController _passCtrl = TextEditingController();
  String _path = '/';
  final List<String> _stack = [];
  List<DirEntry> _entries = [];
  bool _loading = false;
  String? _error;

  @override
  void dispose() {
    final s = _session;
    if (s != null) webdavDisconnect(id: s.id);
    _urlCtrl.dispose();
    _userCtrl.dispose();
    _passCtrl.dispose();
    super.dispose();
  }

  Future<void> _connect() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final s = await webdavConnect(
        url: _urlCtrl.text.trim(),
        username: _userCtrl.text.trim(),
        password: _passCtrl.text,
      );
      setState(() {
        _session = s;
        _path = s.root;
        _stack.clear();
      });
      await _list(s.root);
    } catch (e) {
      setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _list(String path) async {
    final s = _session;
    if (s == null) return;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final entries = await webdavList(session: s.id, path: path);
      if (!mounted) return;
      setState(() {
        _path = path;
        _entries = entries
            .where((e) =>
                e.isDir ||
                e.name.toLowerCase().endsWith('.cbz') ||
                e.name.toLowerCase().endsWith('.zip'))
            .toList();
      });
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  void _openEntry(DirEntry e) {
    if (e.isDir) {
      _stack.add(_path);
      _list(e.path);
    } else {
      Navigator.of(context).push(
        MaterialPageRoute(
          builder: (_) => ReaderPage(
            path: e.path,
            title: e.name,
            webdavSession: _session!.id,
          ),
        ),
      );
    }
  }

  void _goUp() {
    if (_stack.isNotEmpty) {
      _list(_stack.removeLast());
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('WebDAV 书源'),
        actions: [
          if (_session != null)
            IconButton(
              icon: const Icon(Icons.link_off),
              tooltip: '断开连接',
              onPressed: () {
                final s = _session;
                if (s != null) webdavDisconnect(id: s.id);
                setState(() {
                  _session = null;
                  _entries = [];
                  _path = '/';
                  _stack.clear();
                  _error = null;
                });
              },
            ),
        ],
      ),
      body: _session == null ? _connectForm() : _browser(),
    );
  }

  Widget _connectForm() {
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 420),
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: _urlCtrl,
                decoration: const InputDecoration(
                  labelText: '服务器地址',
                  hintText: 'https://nas.example.com:5006/dav',
                  border: OutlineInputBorder(),
                  prefixIcon: Icon(Icons.dns),
                ),
                keyboardType: TextInputType.url,
              ),
              const SizedBox(height: 12),
              TextField(
                controller: _userCtrl,
                decoration: const InputDecoration(
                  labelText: '用户名',
                  border: OutlineInputBorder(),
                  prefixIcon: Icon(Icons.person),
                ),
              ),
              const SizedBox(height: 12),
              TextField(
                controller: _passCtrl,
                obscureText: true,
                decoration: const InputDecoration(
                  labelText: '密码',
                  border: OutlineInputBorder(),
                  prefixIcon: Icon(Icons.lock),
                ),
                onSubmitted: (_) => _connect(),
              ),
              const SizedBox(height: 20),
              if (_error != null)
                Padding(
                  padding: const EdgeInsets.only(bottom: 12),
                  child: Text(_error!, style: const TextStyle(color: Colors.redAccent)),
                ),
              SizedBox(
                width: double.infinity,
                child: FilledButton.icon(
                  onPressed: _loading ? null : _connect,
                  icon: const Icon(Icons.cloud_done),
                  label: Text(_loading ? '连接中…' : '连接'),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _browser() {
    return Column(
      children: [
        Material(
          color: Colors.black26,
          child: ListTile(
            dense: true,
            leading: IconButton(
              icon: const Icon(Icons.arrow_upward),
              tooltip: '上级目录',
              onPressed: _stack.isEmpty ? null : _goUp,
            ),
            title: Text(_path, maxLines: 1, overflow: TextOverflow.ellipsis),
            trailing: IconButton(
              icon: const Icon(Icons.refresh),
              tooltip: '刷新',
              onPressed: () => _list(_path),
            ),
          ),
        ),
        if (_error != null)
          Padding(
            padding: const EdgeInsets.all(8),
            child: Text(_error!, style: const TextStyle(color: Colors.redAccent)),
          ),
        Expanded(
          child: _loading
              ? const Center(child: CircularProgressIndicator())
              : _entries.isEmpty
                  ? const Center(child: Text('(此目录无漫画)'))
                  : ListView.builder(
                      itemCount: _entries.length,
                      itemBuilder: (context, i) {
                        final e = _entries[i];
                        return ListTile(
                          leading: Icon(
                            e.isDir ? Icons.folder : Icons.menu_book,
                            color: e.isDir ? Colors.amber : null,
                          ),
                          title: Text(e.name, maxLines: 1, overflow: TextOverflow.ellipsis),
                          subtitle: e.isDir ? null : Text(fmtSize(e.size)),
                          onTap: () => _openEntry(e),
                        );
                      },
                    ),
        ),
      ],
    );
  }
}
