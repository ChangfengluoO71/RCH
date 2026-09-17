import 'package:app/src/rust/api/book.dart';
import 'package:flutter/material.dart';

/// A folder selected from the 115 file tree.
class Cloud115FolderChoice {
  final String id;
  final String name;

  const Cloud115FolderChoice({required this.id, required this.name});

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is Cloud115FolderChoice && id == other.id && name == other.name;

  @override
  int get hashCode => Object.hash(id, name);
}

/// Lets the user navigate a 115 directory tree and choose the current folder.
///
/// The widget deliberately receives a listing callback instead of creating a
/// 115 session itself. This keeps authentication/session lifetime in the
/// caller and makes the picker reusable for both add and edit flows.
class Cloud115FolderPickerDialog extends StatefulWidget {
  final Future<List<DirEntry>> Function(String path) listDirectory;
  final String initialPath;
  final String? initialName;

  const Cloud115FolderPickerDialog({
    super.key,
    required this.listDirectory,
    this.initialPath = '0',
    this.initialName,
  });

  @override
  State<Cloud115FolderPickerDialog> createState() =>
      _Cloud115FolderPickerDialogState();
}

class _Cloud115FolderLocation {
  final String id;
  final String name;

  const _Cloud115FolderLocation({required this.id, required this.name});
}

class _Cloud115FolderPickerDialogState
    extends State<Cloud115FolderPickerDialog> {
  late final List<_Cloud115FolderLocation> _locations;
  List<DirEntry> _folders = const [];
  bool _loading = true;
  String? _error;

  _Cloud115FolderLocation get _current => _locations.last;

  @override
  void initState() {
    super.initState();
    final initialPath = widget.initialPath.trim().isEmpty
        ? '0'
        : widget.initialPath.trim();
    _locations = [
      _Cloud115FolderLocation(
        id: initialPath,
        name: widget.initialName?.trim().isNotEmpty == true
            ? widget.initialName!.trim()
            : (initialPath == '0' ? '115 网盘根目录' : '当前文件夹'),
      ),
    ];
    _loadCurrent();
  }

  Future<void> _loadCurrent() async {
    if (mounted) {
      setState(() {
        _loading = true;
        _error = null;
      });
    }
    try {
      final entries = await widget.listDirectory(_current.id);
      if (!mounted) return;
      setState(() {
        _folders = entries
            .where((entry) => entry.isDir && entry.path.trim().isNotEmpty)
            .toList(growable: false);
        _loading = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _folders = const [];
        _loading = false;
        _error = '$e';
      });
    }
  }

  Future<void> _enter(_Cloud115FolderLocation folder) async {
    _locations.add(folder);
    await _loadCurrent();
  }

  Future<void> _goUp() async {
    if (_locations.length <= 1) return;
    _locations.removeLast();
    await _loadCurrent();
  }

  Future<void> _goRoot() async {
    if (_current.id == '0') return;
    _locations
      ..clear()
      ..add(const _Cloud115FolderLocation(id: '0', name: '115 网盘根目录'));
    await _loadCurrent();
  }

  void _chooseCurrent() {
    Navigator.of(
      context,
    ).pop(Cloud115FolderChoice(id: _current.id, name: _current.name));
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final screen = MediaQuery.sizeOf(context);
    final dialogWidth = (screen.width - 96).clamp(240.0, 420.0).toDouble();
    final dialogHeight = (screen.height * 0.55).clamp(220.0, 420.0).toDouble();
    return AlertDialog(
      scrollable: true,
      title: const Text('选择 115 漫画文件夹'),
      content: SizedBox(
        width: dialogWidth,
        height: dialogHeight,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                IconButton(
                  tooltip: '返回上级',
                  onPressed: _locations.length > 1 && !_loading ? _goUp : null,
                  icon: const Icon(Icons.arrow_back),
                ),
                IconButton(
                  tooltip: '返回网盘根目录',
                  onPressed: _current.id != '0' && !_loading ? _goRoot : null,
                  icon: const Icon(Icons.home_outlined),
                ),
                Expanded(
                  child: Text(
                    '${_current.name}（${_current.id}）',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.titleSmall,
                  ),
                ),
              ],
            ),
            const Divider(height: 1),
            const SizedBox(height: 8),
            Expanded(child: _body()),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: _loading ? null : _chooseCurrent,
          child: const Text('选择此文件夹'),
        ),
      ],
    );
  }

  Widget _body() {
    if (_loading) return const Center(child: CircularProgressIndicator());
    if (_error != null) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(_error!, textAlign: TextAlign.center),
            const SizedBox(height: 12),
            OutlinedButton(onPressed: _loadCurrent, child: const Text('重试')),
          ],
        ),
      );
    }
    if (_folders.isEmpty) {
      return const Center(child: Text('此文件夹下没有子文件夹'));
    }
    return ListView.builder(
      itemCount: _folders.length,
      itemBuilder: (context, index) {
        final entry = _folders[index];
        final folder = _Cloud115FolderLocation(
          id: entry.path,
          name: entry.name.trim().isEmpty ? entry.path : entry.name,
        );
        return ListTile(
          leading: const Icon(Icons.folder, color: Colors.amber),
          title: Text(folder.name),
          subtitle: Text(folder.id),
          trailing: const Icon(Icons.chevron_right),
          onTap: () => _enter(folder),
        );
      },
    );
  }
}
