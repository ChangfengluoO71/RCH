import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/ai.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
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
  final PhotoViewController _photoCtrl = PhotoViewController();
  final TransformationController _dualZoomCtrl = TransformationController();
  final TransformationController _webtoonZoomCtrl = TransformationController();
  final FocusNode _focus = FocusNode(); final ScrollController _webtoonCtrl = ScrollController();
  late ReadMode _mode; late bool _invert; late DualPageMode _dual; late int _gap; late bool _skipCover;
  late KeyBinds _keys;

  @override void initState() { super.initState();
    final g=LibraryStore.instance.settings;
    _mode=g.readMode; _invert=g.invertTap; _dual=g.dualPageMode; _gap=g.dualPageGap; _skipCover=g.skipFrontCover; _keys=g.keys;
    _open(); }

  Future<void> _open() async { try {
    if (widget.webdavSession != null) {
      // WebDAV: 轮询下载进度
      _startPollingProgress();
      final b = await openWebdavBook(session: widget.webdavSession!, path: widget.path);
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
    _ensure(_page); _ensure(_page+1); _ensure(_page+2);
  } catch(e) { if (mounted) setState(() { _error = '$e'; _downloadProgress = null; }); } }

  /// 轮询 WebDAV 下载进度,每 300ms 一次,直到 _book 出现或状态变化。
  void _startPollingProgress() {
    _downloadProgress = 0.0;
    Future.doWhile(() async {
      if (_downloadProgress == null || _book != null || !mounted) return false;
      await Future.delayed(const Duration(milliseconds: 300));
      if (_downloadProgress == null || _book != null || !mounted) return false;
      try {
        final p = await webdavDownloadProgress(session: widget.webdavSession!);
        if (mounted) setState(() { _downloadProgress = p; });
      } catch (_) {}
      return _downloadProgress != null && _book == null && mounted;
    });
  }

  void _ensure(int i) { final b=_book; if(b==null||i<0||i>=b.pageCount)return;
    if(_bytes.containsKey(i)||_loading.contains(i))return;_loading.add(i);
    bookPage(handle: b.handle, index: i).then((d) async {
      if (!mounted) return;
      if (widget.skipAiCache) {
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
    if(n!=_page){setState(()=>_page=n);_photoCtrl.reset();_dualZoomCtrl.value=Matrix4.identity();for(var i=n-2;i<=n+2;i++){_ensure(i);}
    final s=widget.source;if(s!=null){await LibraryStore.instance.recordRead(source:s,path:widget.path,title:widget.title,page:n);}}}

  // ---- 缩放(仅 +/-/0 键,无滚轮) ----
  void _zoomIn() => _zoomBy(1.25);
  void _zoomOut() => _zoomBy(1 / 1.25);
  void _zoomReset() {
    if (_mode == ReadMode.webtoon) { _webtoonZoomCtrl.value = Matrix4.identity(); }
    else if (_isDual()) { _dualZoomCtrl.value = Matrix4.identity(); }
    else { _photoCtrl.reset(); }
  }
  void _zoomBy(double f) {
    if (_mode == ReadMode.webtoon) { _zoomIV(_webtoonZoomCtrl, f); }
    else if (_isDual()) { _zoomIV(_dualZoomCtrl, f); }
    else { final cur = _photoCtrl.scale ?? 1.0; _photoCtrl.scale = (cur * f).clamp(0.5, 8.0); }
  }
  void _zoomIV(TransformationController c, double f) {
    final cur = c.value.getMaxScaleOnAxis();
    final next = (cur * f).clamp(1.0, 4.0);
    c.value = Matrix4.identity()..scaleByDouble(next, next, next, 1.0);
  }
  void _showJumpDialog() { final b=_book;if(b==null)return;final ctrl=TextEditingController();
    showDialog(context:context,builder:(ctx)=>AlertDialog(title:const Text('跳转到页码'),content:TextField(controller:ctrl,keyboardType:TextInputType.number,autofocus:true,decoration:const InputDecoration(hintText:'输入页码',border:OutlineInputBorder()),onSubmitted:(v){_doJump(v,ctrl,ctx);}),actions:[TextButton(onPressed:()=>Navigator.of(ctx).pop(),child:const Text('取消')),FilledButton(onPressed:(){_doJump(ctrl.text,ctrl,ctx);},child:const Text('跳转'))]));}
  void _doJump(String v, TextEditingController ctrl, BuildContext ctx) { final b=_book;if(b==null)return;final p=int.tryParse(v.trim());if(p!=null){final n=(p-1).clamp(0,b.pageCount-1);setState(()=>_page=n);_photoCtrl.reset();_dualZoomCtrl.value=Matrix4.identity();_ensure(n-2);_ensure(n-1);_ensure(n);_ensure(n+1);_ensure(n+2);}Navigator.of(ctx).pop();}

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

  @override void dispose(){final b=_book;if(b!=null)closeBook(handle:b.handle);_photoCtrl.dispose();_dualZoomCtrl.dispose();_webtoonZoomCtrl.dispose();_focus.dispose();_webtoonCtrl.dispose();super.dispose();}

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

  Widget _buildImage(Uint8List bytes)=> PhotoView(controller:_photoCtrl,imageProvider:ResizeImage(MemoryImage(bytes),width:2000),backgroundDecoration:const BoxDecoration(color:Colors.black),initialScale:PhotoViewComputedScale.contained,minScale:PhotoViewComputedScale.contained,maxScale:PhotoViewComputedScale.covered*8);

  Widget _buildMangaOrComic() { final bytes=_bytes[_page]; final isManga=_mode==ReadMode.manga;
    VoidCallback leftAction =isManga?(_invert?_back:_forward):(_invert?_forward:_back);
    VoidCallback rightAction=isManga?(_invert?_forward:_back):(_invert?_back:_forward);
    final (leftPg,rightPg)=pairOf(_page); Widget pageView;
    if(bytes==null){_ensure(_page);pageView=const Center(child:CircularProgressIndicator());}
    else if(rightPg!=null){_ensure(leftPg);_ensure(rightPg);final lB=_bytes[leftPg],rB=_bytes[rightPg];pageView=lB!=null?_buildPair(leftBytes:lB,leftIdx:leftPg,rightBytes:rB,rightIdx:rightPg):const Center(child:CircularProgressIndicator());}
    else{pageView=_buildImage(bytes);}
    return Stack(children:[
      Positioned.fill(child:pageView),
      Positioned(left:0,top:0,bottom:0,width:80,child:GestureDetector(behavior:HitTestBehavior.opaque,onTap:leftAction)),
      Positioned(right:0,top:0,bottom:0,width:80,child:GestureDetector(behavior:HitTestBehavior.opaque,onTap:rightAction)),
      if(_loading.isNotEmpty)const Positioned(top:8,right:8,child:SizedBox(width:20,height:20,child:CircularProgressIndicator(strokeWidth:2))),
    ]);
  }

  Widget _buildPair({required Uint8List leftBytes, required int leftIdx, Uint8List? rightBytes, required int rightIdx}) {
    final isManga=_mode==ReadMode.manga;_ensure(rightIdx);
    return Center(child: InteractiveViewer(
      transformationController: _dualZoomCtrl, minScale: 1.0, maxScale: 4.0,
      scaleEnabled: false, panEnabled: false,
      child: LayoutBuilder(builder:(context,c){final div=_gap.clamp(0,20);final halfW=((c.maxWidth-div)/2).round().clamp(1,4096);
        Widget tile(Uint8List b)=>ClipRect(child:FittedBox(fit:BoxFit.contain,child:SizedBox(width:halfW.toDouble(),child:Image(image:ResizeImage(MemoryImage(b),width:halfW),fit:BoxFit.contain))));
        Widget leftWidget=tile(leftBytes);Widget rightWidget=rightBytes!=null?tile(rightBytes):const Center(child:SizedBox(width:24,height:24,child:CircularProgressIndicator(strokeWidth:2)));
        return Row(mainAxisSize:MainAxisSize.min,children:[if(isManga)rightWidget,if(isManga&&div>0)SizedBox(width:div.toDouble()),leftWidget,if(!isManga&&div>0)SizedBox(width:div.toDouble()),if(!isManga)rightWidget,]);
      }),
    ));
  }

  // ---- 条漫 ----
  Widget _buildWebtoon(){final b=_book;if(b==null)return const Center(child:CircularProgressIndicator());
    return LayoutBuilder(builder:(context,c){final vw=c.maxWidth;
      return InteractiveViewer(
        transformationController: _webtoonZoomCtrl,
        minScale: 1.0, maxScale: 4.0,
        scaleEnabled: false, panEnabled: false,
        child: ListView.builder(controller:_webtoonCtrl,itemCount:b.pageCount,itemBuilder:(context,i){final bytes=_bytes[i];
          if(bytes==null){_ensure(i);return const SizedBox(height:200,child:Center(child:CircularProgressIndicator()));}
          return GestureDetector(onTap:()async{if(_page!=i){setState(()=>_page=i);final s=widget.source;if(s!=null){await LibraryStore.instance.recordRead(source:s,path:widget.path,title:widget.title,page:i);}}},child:Image(image:ResizeImage(MemoryImage(bytes),width:vw.toInt()),fit:BoxFit.fitWidth),);
        },),
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
      items: const [
        PopupMenuItem(value: 'settings', child: ListTile(leading: Icon(Icons.tune), title: Text('阅读设置'), dense: true)),
        PopupMenuItem(value: 'ai', child: ListTile(leading: Icon(Icons.auto_fix_high), title: Text('AI 超分 (2x)'), dense: true)),
      ],
    ).then((value) {
      if (value == 'settings') _showSettings();
      if (value == 'ai') _doAiSuperResolve();
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
  void _showSettings(){showModalBottomSheet(context:context,builder:(ctx)=>StatefulBuilder(builder:(ctx,ss)=>Padding(padding:EdgeInsets.fromLTRB(20,12,20,24),child:Column(mainAxisSize:MainAxisSize.min,crossAxisAlignment:CrossAxisAlignment.start,children:[
    Center(child:Container(width:36,height:4,decoration:BoxDecoration(color:Colors.white24,borderRadius:BorderRadius.circular(2)))),const SizedBox(height:16),
    const Text('阅读设置(仅对本会话)',style:TextStyle(fontSize:16,fontWeight:FontWeight.w600)),const SizedBox(height:12),
    const Text('阅读模式'),const SizedBox(height:6),
    SegmentedButton<ReadMode>(segments:ReadMode.values.map((r)=>ButtonSegment(value:r,label:Text(r.label))).toList(),selected:{_mode},onSelectionChanged:(vs){ss((){});setState((){_mode=vs.first;});}),
    const SizedBox(height:16),const Text('双页拼接'),const SizedBox(height:6),
    SegmentedButton<DualPageMode>(segments:DualPageMode.values.map((d)=>ButtonSegment(value:d,label:Text(d.label))).toList(),selected:{_dual},onSelectionChanged:(vs){ss((){});setState((){_dual=vs.first;});}),
    const SizedBox(height:8),Row(children:[const Text('拼接间隙:'),SizedBox(width:120,child:Slider(value:_gap.toDouble(),min:0,max:20,divisions:20,label:'${_gap}px',onChanged:(v){ss((){});setState((){_gap=v.toInt();});})),Text('${_gap}px')]),
    const SizedBox(height:10),Row(children:[const Text('首页单独显示(不参与拼接)'),const Spacer(),Switch(value:_skipCover,onChanged:(v){ss((){});setState((){_skipCover=v;});})]),
    const SizedBox(height:16),SwitchListTile(title:const Text('日漫模式点击区反向'),subtitle:const Text('打开后右侧区域变为前进'),dense:true,contentPadding:EdgeInsets.zero,value:_invert,onChanged:(v){ss((){});setState((){_invert=v;});}),
    const SizedBox(height:16),const Text('🤖 AI 超分',style:TextStyle(color:Colors.white38,fontSize:12)),const SizedBox(height:6),
    const SizedBox(width:double.infinity,child:Card(child:Padding(padding:EdgeInsets.all(12),child:Text('右键当前页选择 \'AI 超分 (2x)\' 即可端侧推理放大图片，已启用。',style:TextStyle(fontSize:12,color:Colors.white54))))),
  ]))));}

  @override Widget build(BuildContext context) { final b=_book;
    final isManga=_mode==ReadMode.manga;final (leftPg,rightPg)=pairOf(_page);
    final showL=isManga?Icons.chevron_left:Icons.chevron_right,showR=isManga?Icons.chevron_right:Icons.chevron_left;
    String pageLabel;if(b==null){pageLabel='';}else if(rightPg!=null){final l=leftPg+1,r=rightPg+1;pageLabel=isManga?'$r-$l / ${b.pageCount}':'$l-$r / ${b.pageCount}';}else{pageLabel='${_page+1} / ${b.pageCount}';}
    return Scaffold(
      appBar:AppBar(title:GestureDetector(onTap:_showJumpDialog,child:Text(b==null?widget.title:'${b.title}  ($pageLabel)',maxLines:1,overflow:TextOverflow.ellipsis)),actions:[IconButton(icon:const Icon(Icons.tune),tooltip:'阅读设置',onPressed:_showSettings)]),
      body:Focus(focusNode:_focus,autofocus:true,onKeyEvent:_onKey,child:GestureDetector(onSecondaryTapUp:_onRightClick,child:_buildBody())),
      bottomNavigationBar:_mode==ReadMode.webtoon||b==null?null:SafeArea(child:Padding(padding:EdgeInsets.symmetric(vertical:2),child:Row(mainAxisAlignment:MainAxisAlignment.center,children:[IconButton(icon:Icon(showL),onPressed:_back),GestureDetector(onTap:_showJumpDialog,child:Text(pageLabel,style:const TextStyle(decoration:TextDecoration.underline,decorationStyle:TextDecorationStyle.dotted))),IconButton(icon:Icon(showR),onPressed:_forward)]))),
    );
  }
}
