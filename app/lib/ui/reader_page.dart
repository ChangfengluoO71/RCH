import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/ai.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/ai_upscale_manager.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/common.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:photo_view/photo_view.dart';

class ReaderPage extends StatefulWidget {
  final String path; final String title; final BigInt? webdavSession;
  final BookSource? source; final int initialPage; final bool skipAiCache;
  const ReaderPage({super.key, required this.path, required this.title,
    this.webdavSession, this.source, this.initialPage = 0, this.skipAiCache = false,});
  @override State<ReaderPage> createState() => _ReaderPageState();
}

class _ReaderPageState extends State<ReaderPage> {
  BookInfo? _book; int _page=0; String? _error;
  /// WebDAV 下载进度: 0.0~1.0, null=非 WebDAV 或已完成。
  double? _downloadProgress;
  final Map<int, Uint8List> _bytes = {}; final Set<int> _loading = {};
  bool _aiProcessing = false;
  bool _useAiVersion = true;
  bool _rotationMode = false; // 右键「界面旋转」进入旋转模式
  final Map<int, int> _rotations = {}; // pageIndex -> 度数(0/90/180/270)
  /// 单页模式下每个页面独立的缩放控制器。PageView 滑动时新旧页会同时挂载,
  /// 共用控制器会导致新页图片加载回写缩放时旧页跟着跳动。
  final Map<int, PhotoViewController> _photoCtrls = {};
  final Map<int, PhotoViewScaleStateController> _scaleStateCtrls = {};
  final TransformationController _dualZoomCtrl = TransformationController();
  final TransformationController _webtoonZoomCtrl = TransformationController();
  PageController? _pageCtrl;
  bool _dualZoomed = false; // 双页模式已放大(>1)时接管拖拽,否则让给 PageView 翻页
  final FocusNode _focus = FocusNode(); final ScrollController _webtoonCtrl = ScrollController();
  late ReadMode _mode; late bool _invert; late DualPageMode _dual; late int _gap; late bool _skipCover;
  late KeyBinds _keys;
  /// 进入阅读器时是否为紧凑（手机）布局：退出时据此恢复竖屏锁定或保持可旋转。
  bool _compactAtOpen = true;
  bool _orientationCaptured = false;

  // ---- 视口页 ↔ 真实页映射(双页模式一视口对应两页) ----
  ReaderPaging get _paging => ReaderPaging(
        dual: _dual != DualPageMode.off,
        skipCover: _skipCover,
        pageCount: _book?.pageCount ?? 1,
      );
  int _viewCount() => _paging.viewCount;
  int _viewOfPage(int p) => _paging.viewOfPage(p);
  int _pageOfView(int v) => _paging.pageOfView(v);

  /// 重建 PageController(书打开后、阅读设置变更后调用),保证初始视口与 _page 一致。
  void _recreatePageCtrl() {
    final old = _pageCtrl;
    final vc = _viewCount();
    _pageCtrl = PageController(
      initialPage: _book == null || vc <= 0 ? 0 : _viewOfPage(_page).clamp(0, vc - 1),
    );
    if (old != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) { if (mounted) old.dispose(); });
    }
  }

  PhotoViewController _photoCtrlOf(int page) =>
      _photoCtrls.putIfAbsent(page, () => PhotoViewController());
  PhotoViewScaleStateController _scaleStateCtrlOf(int page) =>
      _scaleStateCtrls.putIfAbsent(page, () => PhotoViewScaleStateController());

  /// 释放离开视口窗口(±1)的页面对应缩放控制器,防止控制器无限增长。
  void _disposeDistantPhotoCtrls(int center) {
    final keep = <int>{for (var i = center - 1; i <= center + 1; i++) i};
    _photoCtrls.removeWhere((p, c) {
      if (keep.contains(p)) return false;
      c.dispose();
      return true;
    });
    _scaleStateCtrls.removeWhere((p, c) {
      if (keep.contains(p)) return false;
      c.dispose();
      return true;
    });
  }

  void _onDualZoomChanged() {
    final zoomed = _dualZoomCtrl.value.getMaxScaleOnAxis() > 1.01;
    if (zoomed != _dualZoomed) setState(() => _dualZoomed = zoomed);
  }

  /// 双击在 1x / 2x 之间切换（条漫、双页模式；单页 PhotoView 自带双击缩放）。
  void _toggleZoomByDoubleTap(TransformationController c) {
    final zoomed = c.value.getMaxScaleOnAxis() > 1.01;
    c.value = zoomed
        ? Matrix4.identity()
        : (Matrix4.identity()..scaleByDouble(2.0, 2.0, 2.0, 1.0));
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!_orientationCaptured) {
      _orientationCaptured = true;
      _compactAtOpen = isCompact(context);
    }
  }

  @override void initState() { super.initState();
    _dualZoomCtrl.addListener(_onDualZoomChanged);
    if (defaultTargetPlatform == TargetPlatform.android) {
      SystemChrome.setPreferredOrientations(DeviceOrientation.values);
    }
    final g=LibraryStore.instance.settings;
    _mode=g.readMode; _invert=g.invertTap; _dual=g.dualPageMode; _gap=g.dualPageGap; _skipCover=g.skipFrontCover; _keys=g.keys;
    final s0 = widget.source;
    if (s0 != null) {
      _rotations.addAll(LibraryStore.instance.metaOf(s0, widget.path).rotations);
    }
    _open();
    AiUpscaleManager.instance.addListener(_onAiManager);
    final s = widget.source;
    // 延迟到本帧构建结束后再通知 AI 管理器：setReadingBook 会 notifyListeners，
    // 若在 initState（Navigator push 构建期间）同步触发，详情页监听器 setState 会
    // 抛 "setState() called during build"。
    final bk = s == null ? null : bookKeyOf(s.type, s.id, widget.path);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) AiUpscaleManager.instance.setReadingBook(bk);
    });
  }

  void _onAiManager() {
    final s = widget.source;
    if (s == null) return;
    final bookKey = bookKeyOf(s.type, s.id, widget.path);
    final m = AiUpscaleManager.instance;
    if (m.forceAiVersionBookKey == bookKey) {
      m.consumeForceAiVersion();
      if (!_useAiVersion) _toggleAiVersion();
    }
  }

  /// 原版 / 超分版本切换：清空当前视口页并重新加载，页码不变。
  void _toggleAiVersion() {
    setState(() {
      _useAiVersion = !_useAiVersion;
      for (var i = _page - 1; i <= _page + 2; i++) {
        if (i >= 0) {
          _bytes.remove(i);
          _loading.remove(i);
        }
      }
    });
    _photoCtrlOf(_page).reset();
    _scaleStateCtrlOf(_page).reset();
    _dualZoomCtrl.value = Matrix4.identity();
    _webtoonZoomCtrl.value = Matrix4.identity();
    _ensure(_page);
    if (_isDual()) _ensure(_page + 1);
    _ensure(_page + 1);
    _ensure(_page + 2);
  }

  // ---- 页面旋转(每页独立,右键「界面旋转」进入) ----
  int _rotationOf(int page) => _rotations[page] ?? 0;

  void _toggleRotationMode() {
    if (_mode == ReadMode.webtoon) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('条漫模式暂不支持旋转')),
      );
      return;
    }
    setState(() => _rotationMode = !_rotationMode);
  }

  void _rotatePage(int page) {
    final next = (_rotationOf(page) + 90) % 360;
    setState(() {
      if (next == 0) {
        _rotations.remove(page);
      } else {
        _rotations[page] = next;
      }
    });
    final s = widget.source;
    if (s == null) return;
    final m = LibraryStore.instance.metaOf(s, widget.path);
    m.rotations
      ..clear()
      ..addAll(_rotations);
    LibraryStore.instance.updateMeta(m);
  }

  Future<void> _open() async { try {
    final src = widget.source;
    final strategy = LibraryStore.instance.settings.bookOpenStrategy.name;
    if (src?.isWebDav == true && widget.webdavSession != null) {
      // 远程(WebDAV/SFTP)会话: 轮询下载进度后打开
      _startPollingProgress(
          progressFn: () => webdavDownloadProgress(session: widget.webdavSession!));
      final b = await openWebdavBook(
          session: widget.webdavSession!, path: widget.path, strategy: strategy);
      _downloadProgress = null;
      if (!mounted) return;
      setState(() { _book = b; });
    } else if (src?.isSftp == true && widget.webdavSession != null) {
      _startPollingProgress(
          progressFn: () => sftpDownloadProgress(session: widget.webdavSession!));
      final b = await openSftpBook(
          session: widget.webdavSession!, path: widget.path, strategy: strategy);
      _downloadProgress = null;
      if (!mounted) return;
      setState(() { _book = b; });
    } else if (src?.isBaidu == true && widget.webdavSession != null) {
      _startPollingProgress(
          progressFn: () => baiduDownloadProgress(session: widget.webdavSession!));
      final b = await openBaiduBook(
          session: widget.webdavSession!, path: widget.path, strategy: strategy);
      _downloadProgress = null;
      if (!mounted) return;
      setState(() { _book = b; });
    } else if (src?.is115 == true && widget.webdavSession != null) {
      _startPollingProgress(
          progressFn: () => cloud115DownloadProgressFor(src!,
              session: widget.webdavSession!));
      final b = await openCloud115BookFor(src!,
          session: widget.webdavSession!, path: widget.path, strategy: strategy);
      _downloadProgress = null;
      if (!mounted) return;
      setState(() { _book = b; });
    } else if (src?.isQuark == true && widget.webdavSession != null) {
      _startPollingProgress(
          progressFn: () => quarkDownloadProgress(session: widget.webdavSession!));
      final b = await openQuarkBook(
          session: widget.webdavSession!, path: widget.path, strategy: strategy);
      _downloadProgress = null;
      if (!mounted) return;
      setState(() { _book = b; });
    } else {
      final b = await openLocalBook(path: widget.path);
      if (!mounted) return;
      setState(() { _book = b; });
    }
    if (!mounted) return;
    setState(() { _page = widget.initialPage.clamp(0, _book!.pageCount > 0 ? _book!.pageCount - 1 : 0); });
    _recreatePageCtrl();
    _ensure(_page); _ensure(_page+1); _ensure(_page+2);
  } catch(e) { if (mounted) setState(() { _error = '$e'; _downloadProgress = null; }); } }

  /// 轮询远程下载进度(WebDAV / SFTP / 百度 / 115),每 300ms 一次,直到 _book 出现或状态变化。
  void _startPollingProgress({required Future<double> Function() progressFn}) {
    _downloadProgress = 0.0;
    Future.doWhile(() async {
      if (_downloadProgress == null || _book != null || !mounted) return false;
      await Future.delayed(const Duration(milliseconds: 300));
      if (_downloadProgress == null || _book != null || !mounted) return false;
      try {
        final p = await progressFn();
        if (mounted) setState(() { _downloadProgress = p; });
      } catch (_) {}
      return _downloadProgress != null && _book == null && mounted;
    });
  }

  void _ensure(int i) { final b=_book; if(b==null||i<0||i>=b.pageCount)return;
    if(_bytes.containsKey(i)||_loading.contains(i))return;_loading.add(i);
    bookPage(handle: b.handle, index: i).then((d) async {
      if (!mounted) return;
      if (widget.skipAiCache || !_useAiVersion) {
        if (mounted) setState(() { _bytes[i] = d; _loading.remove(i); });
        return;
      }
      try {
        final ai = await lookupCache(pageBytes: d.toList(), scale: 2);
        if (ai != null && mounted) {
          setState(() { _bytes[i] = ai; _loading.remove(i); });
        } else if (mounted) {
          setState(() { _bytes[i] = d; _loading.remove(i); });
        }
      } catch (_) {
        if (mounted) setState(() { _bytes[i] = d; _loading.remove(i); });
      }
    }).catchError((_) {
      if (mounted) setState(() => _loading.remove(i));
    });
  }

  // ---- 双页配对 ----
  (int,int?) pairOf(int page) { if(_dual==DualPageMode.off)return(page,null);final b=_book;if(b==null)return(page,null);
    final isManga=_mode==ReadMode.manga;if(_skipCover&&page==0)return(0,null);
    int base=_skipCover?1+((page-1)-(page-1)%2):page-(page%2);final first=base,second=base+1;
    final left=isManga?second:first,right=isManga?first:second;if(right>=b.pageCount)return(left,null);return(left,right); }
  bool _isDual() => pairOf(_page).$2 != null;

  // ---- 翻页 ----
  void _forward() { final s=_dual!=DualPageMode.off?2:1; _go(_mode==ReadMode.manga?-s:s); }
  void _back(){ final s=_dual!=DualPageMode.off?2:1; _go(_mode==ReadMode.manga?s:-s); }
  Future<void> _go(int d) async { final b=_book;if(b==null)return;final n=(_page+d).clamp(0,b.pageCount-1);
    if(n==_page)return;
    setState(()=>_page=n);
    _photoCtrlOf(n).reset();_scaleStateCtrlOf(n).reset();_dualZoomCtrl.value=Matrix4.identity();
    _pageCtrl?.animateToPage(_viewOfPage(n),duration:const Duration(milliseconds:220),curve:Curves.easeOutCubic);
    _disposeDistantPhotoCtrls(n);
    for(var i=n-2;i<=n+2;i++){_ensure(i);}
    final s=widget.source;if(s!=null){await LibraryStore.instance.recordRead(source:s,path:widget.path,title:widget.title,page:n);}}

  // ---- 缩放(仅 +/-/0 键,无滚轮) ----
  void _zoomIn() => _zoomBy(1.25);
  void _zoomOut() => _zoomBy(1 / 1.25);
  void _zoomReset() {
    if (_mode == ReadMode.webtoon) { _webtoonZoomCtrl.value = Matrix4.identity(); }
    else if (_isDual()) { _dualZoomCtrl.value = Matrix4.identity(); }
    else { _photoCtrlOf(_page).reset(); _scaleStateCtrlOf(_page).reset(); }
  }
  void _zoomBy(double f) {
    if (_mode == ReadMode.webtoon) { _zoomIV(_webtoonZoomCtrl, f); }
    else if (_isDual()) { _zoomIV(_dualZoomCtrl, f); }
    else { final c = _photoCtrlOf(_page); final cur = c.scale ?? 1.0; c.scale = (cur * f).clamp(0.5, 8.0); }
  }
  void _zoomIV(TransformationController c, double f) {
    final cur = c.value.getMaxScaleOnAxis();
    final next = (cur * f).clamp(1.0, 4.0);
    c.value = Matrix4.identity()..scaleByDouble(next, next, next, 1.0);
  }
  void _showJumpDialog() { final b=_book;if(b==null)return;final ctrl=TextEditingController();
    showDialog(context:context,builder:(ctx)=>AlertDialog(title:const Text('跳转到页码'),content:TextField(controller:ctrl,keyboardType:TextInputType.number,autofocus:true,decoration:const InputDecoration(hintText:'输入页码',border:OutlineInputBorder()),onSubmitted:(v){_doJump(v,ctrl,ctx);}),actions:[TextButton(onPressed:()=>Navigator.of(ctx).pop(),child:const Text('取消')),FilledButton(onPressed:(){_doJump(ctrl.text,ctrl,ctx);},child:const Text('跳转'))]));}
  void _doJump(String v, TextEditingController ctrl, BuildContext ctx) { final b=_book;if(b==null)return;final p=int.tryParse(v.trim());if(p!=null){final n=(p-1).clamp(0,b.pageCount-1);setState(()=>_page=n);_photoCtrlOf(n).reset();_scaleStateCtrlOf(n).reset();_dualZoomCtrl.value=Matrix4.identity();_pageCtrl?.jumpToPage(_viewOfPage(n));_disposeDistantPhotoCtrls(n);_ensure(n-2);_ensure(n-1);_ensure(n);_ensure(n+1);_ensure(n+2);}Navigator.of(ctx).pop();}

  // ---- 键盘(可自定义的 5 个动作) ----
  KeyEventResult _onKey(FocusNode n, KeyEvent e) {
    if (e is! KeyDownEvent) return KeyEventResult.ignored;
    final k = e.logicalKey;
    if (k == _keys.zoomInKey || k == LogicalKeyboardKey.add) { _zoomIn(); return KeyEventResult.handled; }
    if (k == _keys.zoomOutKey || k == LogicalKeyboardKey.numpadSubtract) { _zoomOut(); return KeyEventResult.handled; }
    if (k == _keys.zoomResetKey || k == LogicalKeyboardKey.numpad0) { _zoomReset(); return KeyEventResult.handled; }
    if (_mode == ReadMode.webtoon) {
      if (k == _keys.forwardKey || k == LogicalKeyboardKey.arrowDown || k == LogicalKeyboardKey.space) {
        _webtoonCtrl.animateTo(_webtoonCtrl.offset+400,duration:const Duration(milliseconds:150),curve:Curves.easeOut); return KeyEventResult.handled; }
      if (k == _keys.backKey || k == LogicalKeyboardKey.arrowUp) {
        _webtoonCtrl.animateTo(_webtoonCtrl.offset-400,duration:const Duration(milliseconds:150),curve:Curves.easeOut); return KeyEventResult.handled; }
      return KeyEventResult.ignored;
    }
    bool isManga = _mode == ReadMode.manga;
    if (k == _keys.forwardKey || k == LogicalKeyboardKey.space || k == LogicalKeyboardKey.pageDown) { isManga ? _back() : _forward(); return KeyEventResult.handled; }
    if (k == _keys.backKey || k == LogicalKeyboardKey.pageUp) { isManga ? _forward() : _back(); return KeyEventResult.handled; }
    return KeyEventResult.ignored;
  }

  @override void dispose(){
    AiUpscaleManager.instance.removeListener(_onAiManager);
    AiUpscaleManager.instance.setReadingBook(null);
    if (defaultTargetPlatform == TargetPlatform.android) {
      SystemChrome.setPreferredOrientations(
        _compactAtOpen ? [DeviceOrientation.portraitUp] : DeviceOrientation.values,
      );
    }
    final b=_book;if(b!=null)closeBook(handle:b.handle);
    for (final c in _photoCtrls.values) { c.dispose(); }
    for (final c in _scaleStateCtrls.values) { c.dispose(); }
    _dualZoomCtrl.removeListener(_onDualZoomChanged);
    _dualZoomCtrl.dispose();_webtoonZoomCtrl.dispose();_pageCtrl?.dispose();_focus.dispose();_webtoonCtrl.dispose();super.dispose();
  }

  // ========== 布局 ==========
  /// 构建 body: 下载进度 / 加载中 / 错误 / 阅读视图。
  Widget _buildBody() {
    if (_error != null) {
      return Center(child: Padding(padding: const EdgeInsets.all(16), child: Text('错误: $_error', style: const TextStyle(color: Colors.redAccent))));
    }
    if (_downloadProgress != null) {
      final pct = _downloadProgress!;
      return Center(child: Padding(padding: const EdgeInsets.symmetric(horizontal: 48), child: Column(mainAxisSize: MainAxisSize.min, children: [
        const SizedBox(width: 48, height: 48, child: CircularProgressIndicator(strokeWidth: 3)),
        const SizedBox(height: 20),
        const Text('正在下载漫画...', style: TextStyle(fontSize: 16)),
        const SizedBox(height: 12),
        ClipRRect(
          borderRadius: BorderRadius.circular(4),
          child: LinearProgressIndicator(value: pct, minHeight: 8),
        ),
        const SizedBox(height: 8),
        Text('${(pct * 100).toStringAsFixed(0)}%', style: const TextStyle(fontSize: 18, fontWeight: FontWeight.w600)),
        const SizedBox(height: 4),
        const Text('首次阅读需下载整本,后续秒开', style: TextStyle(fontSize: 11, color: Colors.white38)),
      ])));
    }
    final b = _book;
    if (b == null) return const Center(child: CircularProgressIndicator());
    if (_mode == ReadMode.webtoon) return _buildWebtoon();
    return _buildMangaOrComic();
  }

  Widget _buildImage(Uint8List bytes, int page) {
    final viewer = PhotoView(controller:_photoCtrlOf(page),scaleStateController:_scaleStateCtrlOf(page),imageProvider:ResizeImage(MemoryImage(bytes),width:2000),backgroundDecoration:const BoxDecoration(color:Colors.black),initialScale:PhotoViewComputedScale.contained,minScale:PhotoViewComputedScale.contained,maxScale:PhotoViewComputedScale.covered*8);
    final q = _rotationOf(page) ~/ 90;
    // 未放大时把水平拖拽让给外层 PageView 翻页(photo_view 官方 PageView 适配)。
    return PhotoViewGestureDetectorScope(
      axis: Axis.horizontal,
      child: q == 0 ? viewer : RotatedBox(quarterTurns: q, child: viewer),
    );
  }

  Widget _rotationButton(int page) => Tooltip(
        message: '旋转该页（当前 ${_rotationOf(page)}°）',
        child: Material(
          color: Colors.black54,
          shape: const CircleBorder(),
          child: InkWell(
            customBorder: const CircleBorder(),
            onTap: () => _rotatePage(page),
            child: const Padding(
              padding: EdgeInsets.all(8),
              child: Icon(Icons.rotate_right, size: 22, color: Colors.white),
            ),
          ),
        ),
      );

  /// 日漫/美漫模式：PageView 承载页面实现滑动翻页,点按区域/旋转按钮保留。
  Widget _buildMangaOrComic() {
    final b = _book; if (b == null) return const Center(child: CircularProgressIndicator());
    final ctrl = _pageCtrl; if (ctrl == null) return const Center(child: CircularProgressIndicator());
    return PageView.builder(
      controller: ctrl,
      itemCount: _viewCount(),
      onPageChanged: (v) {
        final p = _pageOfView(v);
        if (p == _page) return;
        setState(() { _page = p; _photoCtrlOf(p).reset(); _scaleStateCtrlOf(p).reset(); _dualZoomCtrl.value = Matrix4.identity(); });
        _disposeDistantPhotoCtrls(p);
        for (var i = p - 2; i <= p + 2; i++) { _ensure(i); }
        final s = widget.source;
        if (s != null) { LibraryStore.instance.recordRead(source: s, path: widget.path, title: widget.title, page: p); }
      },
      itemBuilder: (context, v) => _buildMangaOrComicPage(_pageOfView(v)),
    );
  }

  Widget _buildMangaOrComicPage(int page) { final bytes=_bytes[page]; final isManga=_mode==ReadMode.manga;
    VoidCallback leftAction =isManga?(_invert?_back:_forward):(_invert?_forward:_back);
    VoidCallback rightAction=isManga?(_invert?_forward:_back):(_invert?_back:_forward);
    final (leftPg,rightPg)=pairOf(page); Widget pageView;
    if(bytes==null){_ensure(page);pageView=const Center(child:CircularProgressIndicator());}
    else if(rightPg!=null){_ensure(leftPg);_ensure(rightPg);final lB=_bytes[leftPg],rB=_bytes[rightPg];pageView=lB!=null?_buildPair(leftBytes:lB,leftIdx:leftPg,rightBytes:rB,rightIdx:rightPg):const Center(child:CircularProgressIndicator());}
    else{pageView=_buildImage(bytes,page);}
    return Stack(children:[
      Positioned.fill(child:pageView),
      Positioned(left:0,top:0,bottom:0,width:80,child:GestureDetector(behavior:HitTestBehavior.opaque,onTap:leftAction)),
      Positioned(right:0,top:0,bottom:0,width:80,child:GestureDetector(behavior:HitTestBehavior.opaque,onTap:rightAction)),
      if(_loading.isNotEmpty)const Positioned(top:8,right:8,child:SizedBox(width:20,height:20,child:CircularProgressIndicator(strokeWidth:2))),
      if (_rotationMode) ...[
        if (rightPg == null)
          Positioned(left: 0, right: 0, bottom: 10, child: Center(child: _rotationButton(page)))
        else ...[
          Positioned(left: 12, bottom: 10, child: _rotationButton(leftPg)),
          Positioned(right: 12, bottom: 10, child: _rotationButton(rightPg)),
        ],
      ],
    ]);
  }

  Widget _buildPair({required Uint8List leftBytes, required int leftIdx, Uint8List? rightBytes, required int rightIdx}) {
    final isManga=_mode==ReadMode.manga;_ensure(rightIdx);
    return GestureDetector(
      onDoubleTap: () => _toggleZoomByDoubleTap(_dualZoomCtrl),
      child: Center(child: InteractiveViewer(
        transformationController: _dualZoomCtrl, minScale: 1.0, maxScale: 4.0,
        scaleEnabled: false, panEnabled: _dualZoomed,
        child: LayoutBuilder(builder:(context,c){final div=_gap.clamp(0,20);
          // 向下取整保证 2*halfW+div <= maxWidth，避免双页拼接 Row 亚像素溢出
          // （round() 向上取整会偶发 RIGHT OVERFLOWED BY 0.x PIXELS 遮挡画面）。
          final halfW=((c.maxWidth-div)/2).floor().clamp(1,4096);
          Widget tile(Uint8List b, int idx)=>ClipRect(child:FittedBox(fit:BoxFit.contain,child:RotatedBox(quarterTurns:_rotationOf(idx)~/90,child:SizedBox(width:halfW.toDouble(),child:Image(image:ResizeImage(MemoryImage(b),width:halfW),fit:BoxFit.contain)))));
          Widget leftWidget=tile(leftBytes,leftIdx);Widget rightWidget=rightBytes!=null?tile(rightBytes,rightIdx):const Center(child:SizedBox(width:24,height:24,child:CircularProgressIndicator(strokeWidth:2)));
          return Row(mainAxisSize:MainAxisSize.min,children:[if(isManga)rightWidget,if(isManga&&div>0)SizedBox(width:div.toDouble()),leftWidget,if(!isManga&&div>0)SizedBox(width:div.toDouble()),if(!isManga)rightWidget,]);
        }),
      )),
    );
  }

  // ---- 条漫 ----
  Widget _buildWebtoon(){final b=_book;if(b==null)return const Center(child:CircularProgressIndicator());
    return LayoutBuilder(builder:(context,c){final vw=c.maxWidth;
      // 按设备像素比解码,避免用逻辑宽度解码导致高 DPI 屏幕模糊。
      final decodeW=(vw*MediaQuery.devicePixelRatioOf(context)).ceil().clamp(1,4096);
      return GestureDetector(
        onDoubleTap: () => _toggleZoomByDoubleTap(_webtoonZoomCtrl),
        child: InteractiveViewer(
          transformationController: _webtoonZoomCtrl,
          minScale: 1.0, maxScale: 4.0,
          scaleEnabled: true, panEnabled: false,
          child: ListView.builder(controller:_webtoonCtrl,itemCount:b.pageCount,itemBuilder:(context,i){final bytes=_bytes[i];
            if(bytes==null){_ensure(i);return const SizedBox(height:200,child:Center(child:CircularProgressIndicator()));}
            return GestureDetector(onTap:()async{if(_page!=i){setState(()=>_page=i);final s=widget.source;if(s!=null){await LibraryStore.instance.recordRead(source:s,path:widget.path,title:widget.title,page:i);}}},child:Image(image:ResizeImage(MemoryImage(bytes),width:decodeW),fit:BoxFit.fitWidth),);
          },),
        ),
      );
    });
  }

  // ---- 右键菜单 ----
  void _onRightClick(TapUpDetails details) {
    showMenu<String>(
      position: RelativeRect.fromLTRB(
        details.globalPosition.dx,
        details.globalPosition.dy,
        details.globalPosition.dx + 1,
        details.globalPosition.dy + 1,
      ),
      context: context,
      items: [
        if(!isAndroidPlatform)PopupMenuItem(value: 'ai_version', child: ListTile(leading: Icon(_useAiVersion ? Icons.image_not_supported : Icons.auto_fix_high), title: Text(_useAiVersion ? '使用原版' : '使用超分版本'), dense: true)),
        PopupMenuItem(value: 'settings', child: ListTile(leading: Icon(Icons.tune), title: Text('阅读设置'), dense: true)),
        if(!isAndroidPlatform)PopupMenuItem(value: 'ai', child: ListTile(leading: Icon(Icons.auto_fix_high), title: Text('AI 超分 (2x)'), dense: true)),
        PopupMenuItem(value: 'rotate', child: ListTile(leading: Icon(_rotationMode ? Icons.rotate_left : Icons.rotate_right), title: Text(_rotationMode ? '退出旋转模式' : '界面旋转'), dense: true)),
      ],
    ).then((value) {
      if (value == 'ai_version') _toggleAiVersion();
      if (value == 'settings') _showSettings();
      if (value == 'ai') _doAiSuperResolve();
      if (value == 'rotate') _toggleRotationMode();
    });
  }

  Future<void> _doAiSuperResolve() async {
    if (_aiProcessing) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('AI 超分处理中，请稍候')),
        );
      }
      return;
    }
    _aiProcessing = true;
    try {
    final bytes = _bytes[_page];
    if (bytes == null) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('当前页尚未加载，请等待加载完成')),
        );
      }
      return;
    }
    final b = _book;
    if (b == null) return;

    setState(() {});
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(content: Text('AI 超分处理中...'), duration: Duration(seconds: 2)),
    );
    try {
      final result = await superResolve(pageBytes: bytes, scale: 2);
      if (!mounted) return;
      setState(() {
        _bytes[_page] = result;
      });
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('AI 超分完成 ✓'), duration: Duration(seconds: 2)),
      );
    } catch (e) {
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('AI 超分失败: $e'), duration: const Duration(seconds: 4)),
      );
    }
    } finally {
      _aiProcessing = false;
    }
  }

  // ---- 设置 ----
  void _showSettings(){showModalBottomSheet(context:context,isScrollControlled:true,builder:(ctx)=>StatefulBuilder(builder:(ctx,ss)=>SingleChildScrollView(padding:EdgeInsets.fromLTRB(20,12,20,24),child:Column(mainAxisSize:MainAxisSize.min,crossAxisAlignment:CrossAxisAlignment.start,children:[
    Center(child:Container(width:36,height:4,decoration:BoxDecoration(color:Colors.white24,borderRadius:BorderRadius.circular(2)))),const SizedBox(height:16),
    const Text('阅读设置(仅对本会话)',style:TextStyle(fontSize:16,fontWeight:FontWeight.w600)),const SizedBox(height:12),
    const Text('阅读模式'),const SizedBox(height:6),
    SegmentedButton<ReadMode>(segments:ReadMode.values.map((r)=>ButtonSegment(value:r,label:Text(r.label))).toList(),selected:{_mode},onSelectionChanged:(vs){ss((){});setState((){_mode=vs.first;});_recreatePageCtrl();}),
    const SizedBox(height:16),const Text('双页拼接'),const SizedBox(height:6),
    SegmentedButton<DualPageMode>(segments:DualPageMode.values.map((d)=>ButtonSegment(value:d,label:Text(d.label))).toList(),selected:{_dual},onSelectionChanged:(vs){ss((){});setState((){_dual=vs.first;});_recreatePageCtrl();}),
    const SizedBox(height:8),Row(children:[const Text('拼接间隙:'),SizedBox(width:120,child:Slider(value:_gap.toDouble(),min:0,max:20,divisions:20,label:'${_gap}px',onChanged:(v){ss((){});setState((){_gap=v.toInt();});})),Text('${_gap}px')]),
    const SizedBox(height:10),Row(children:[const Text('首页单独显示(不参与拼接)'),const Spacer(),Switch(value:_skipCover,onChanged:(v){ss((){});setState((){_skipCover=v;});_recreatePageCtrl();})]),
    const SizedBox(height:16),SwitchListTile(title:const Text('日漫模式点击区反向'),subtitle:const Text('打开后右侧区域变为前进'),dense:true,contentPadding:EdgeInsets.zero,value:_invert,onChanged:(v){ss((){});setState((){_invert=v;});}),
    if(!isAndroidPlatform)...[const SizedBox(height:16),const Text('🤖 AI 超分',style:TextStyle(color:Colors.white38,fontSize:12)),const SizedBox(height:6),const SizedBox(width:double.infinity,child:Card(child:Padding(padding:EdgeInsets.all(12),child:Text('右键当前页选择 \'AI 超分 (2x)\' 即可端侧推理放大图片，已启用。',style:TextStyle(fontSize:12,color:Colors.white54)))))],
  ]))));}

  @override Widget build(BuildContext context) { final b=_book;
    final isManga=_mode==ReadMode.manga;final (leftPg,rightPg)=pairOf(_page);
    // 左键=后退(日漫为前进),右键=前进(日漫为后退);箭头一律指向目标方向。
    final showL=Icons.chevron_left,showR=Icons.chevron_right;
    String pageLabel;if(b==null){pageLabel='';}else if(rightPg!=null){final l=leftPg+1,r=rightPg+1;pageLabel=isManga?'$r-$l / ${b.pageCount}':'$l-$r / ${b.pageCount}';}else{pageLabel='${_page+1} / ${b.pageCount}';}
    return PopScope(
      canPop: true,
      child: Scaffold(
      appBar:AppBar(title:GestureDetector(onTap:_showJumpDialog,child:Text(b==null?widget.title:'${b.title}  ($pageLabel)',maxLines:1,overflow:TextOverflow.ellipsis)),actions:[if(!isAndroidPlatform)IconButton(icon:Icon(_useAiVersion ? Icons.auto_fix_high : Icons.image_not_supported),tooltip:_useAiVersion ? '当前为超分版本，点击切换原版' : '当前为原版，点击切换超分版本',onPressed:_toggleAiVersion),IconButton(icon:const Icon(Icons.tune),tooltip:'阅读设置',onPressed:_showSettings)]),
      body:Focus(focusNode:_focus,autofocus:true,onKeyEvent:_onKey,child:GestureDetector(onSecondaryTapUp:_onRightClick,onLongPressStart:(d)=>_onRightClick(TapUpDetails(kind:PointerDeviceKind.touch,globalPosition:d.globalPosition)),child:_buildBody())),
      bottomNavigationBar:_mode==ReadMode.webtoon||b==null?null:SafeArea(child:Padding(padding:EdgeInsets.symmetric(vertical:2),child:Row(mainAxisAlignment:MainAxisAlignment.center,children:[IconButton(icon:Icon(showL),onPressed:_back),GestureDetector(onTap:_showJumpDialog,child:Text(pageLabel,style:const TextStyle(decoration:TextDecoration.underline,decorationStyle:TextDecorationStyle.dotted))),IconButton(icon:Icon(showR),onPressed:_forward)]))),
      ),
    );
  }
}

/// 阅读器「视口 ↔ 真实页」映射。
/// 双页模式下一页视口对应两页;「首页单独显示」时首页再独占一个视口。
class ReaderPaging {
  const ReaderPaging({
    required this.dual,
    required this.skipCover,
    required this.pageCount,
  });

  final bool dual;
  final bool skipCover;
  final int pageCount;

  int get viewCount {
    if (!dual) return pageCount;
    if (pageCount <= 1) return 1;
    return skipCover ? 1 + (pageCount ~/ 2) : (pageCount + 1) ~/ 2;
  }

  /// 真实页(双页模式下为拼接组基准页) → 视口序号。
  int viewOfPage(int page) {
    if (!dual) return page;
    if (skipCover) return page == 0 ? 0 : 1 + ((page - 1) ~/ 2);
    return page ~/ 2;
  }

  /// 视口序号 → 真实基准页。
  int pageOfView(int view) {
    if (!dual) return view;
    if (skipCover) return view == 0 ? 0 : 1 + (view - 1) * 2;
    return view * 2;
  }
}
