import 'package:app/store/automation_coordinator.dart';
import 'package:flutter/material.dart';

/// Single manual recovery action for users who changed a library folder.
///
/// The coordinator still owns the complete catalog -> scrape -> materialize ->
/// sync flow; this widget intentionally exposes no proposal/debug surface in
/// Settings.
class ScrapePanel extends StatefulWidget {
  const ScrapePanel({super.key});

  @override
  State<ScrapePanel> createState() => _ScrapePanelState();
}

class _ScrapePanelState extends State<ScrapePanel> {
  bool _loading = false;

  Future<void> _runScrape() async {
    if (_loading) return;
    setState(() => _loading = true);
    final coordinator = AutomationCoordinator.instance;
    try {
      final run = await coordinator.runScrapeNow();
      if (!mounted) return;
      final message = run == null
          ? coordinator.lastStatus
          : '重新刮削完成：${run.processed}/${run.total}';
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(message),
          backgroundColor: run == null ? Colors.red : null,
        ),
      );
    } catch (error) {
      if (!mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('重新刮削失败：$error')));
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  @override
  Widget build(BuildContext context) => Align(
    alignment: Alignment.centerLeft,
    child: FilledButton.icon(
      onPressed: _loading ? null : _runScrape,
      icon: _loading
          ? const SizedBox(
              width: 16,
              height: 16,
              child: CircularProgressIndicator(strokeWidth: 2),
            )
          : const Icon(Icons.refresh),
      label: const Text('重新刮削'),
    ),
  );
}
