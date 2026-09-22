# WebDAV 协议与服务器/客户端行为调研简报

**主题**：RCH WebDAV 客户端在 "连接/测试连接" 时用 `GET <collection>` + `Range: bytes=0-0` 探测 Range 支持，对 OpenList v4.2.4 的目录 `/dav` 得到 **405**，导致连接整体失败。

**结论速览**：这是 RCH 客户端的探测目标选择错误。规范层面 405 完全合法；该 405 来自 `golang.org/x/net/webdav`（OpenList 内联的 `server/webdav`）在 `GET` 目录时显式返回的 `http.StatusMethodNotAllowed`。**Range 支持必须针对真实文件探测，不能针对 collection**；且不应把探测失败当作致命错误。

---

## 1. RFC 4918（WebDAV）：collection 上的 GET/HEAD 与 405

### 结论
**405 是规范允许且预期之内的响应**。RFC 4918 §9.4 明确保留了 collection 上 GET/HEAD 的语义，但**没有规定**必须返回什么——"may return … or something else altogether"。服务器完全可以不支持 collection 上的 GET。此时按 RFC 7231/9110 §405 的定义，返回 `405 Method Not Allowed` 是标准行为。**规范同时也允许**服务器返回目录列表（200）。

### 证据（confirmed by spec）

**RFC 4918 §9.4 "GET, HEAD for Collections"**（[RFC 4918](https://www.rfc-editor.org/rfc/rfc4918.txt)）：

> The semantics of GET are unchanged when applied to a collection, since GET is defined as, "retrieve whatever information (in the form of an entity) is identified by the Request-URI" [RFC2616]. GET, when applied to a collection, **may return the contents of an "index.html" resource, a human-readable view of the contents of the collection, or something else altogether**. Hence, it is possible that the result of a GET on a collection will bear no correlation to the membership of the collection.
>
> Similarly, since the definition of HEAD is a GET without a response message body, the semantics of HEAD are unmodified when applied to collection resources.

关键点：**§9.4 既未要求成功，也未要求 405**——它把 collection 上 GET 的返回内容完全交给服务器决定。因此 405 *和* 200-with-listing 都合规。

**RFC 4918 §7.1 / 属性定义**（同上，PROPFIND 示例的说明文字）确认了 collection 允许不支持 GET：

> Since **GET is not supported on this resource**, the get* properties (e.g., DAV:getcontentlength) are **not defined** on this resource.

并且 §15.4 `getcontentlength`：

> The DAV:getcontentlength property MUST be defined on any DAV-compliant resource **that returns the Content-Length header in response to a GET**.

即：集合若不对 GET 返回 Content-Length，就**不应**有 `getcontentlength`，这本身是规范预期的状态。

**RFC 7231 §6.5.5 / RFC 9110 §15.5.6 `405 Method Not Allowed`**（[RFC 7231](https://www.rfc-editor.org/rfc/rfc7231.txt)、[RFC 9110](https://www.rfc-editor.org/rfc/rfc9110.txt)）：

> The 405 (Method Not Allowed) status code indicates that **the method received in the request-line is known by the origin server but not supported by the target resource**. The origin server MUST generate an Allow header field in a 405 response containing a list of the target resource's currently supported methods.

**RFC 4918 §10.1 `DAV` 响应头**只声明 compliance class（`1`/`2`/`3` 或扩展 token），**与 Range 无关**。§18 的 Class 1/2/3 要求中也**没有任何** Range 相关义务。

> All DAV-compliant resources MUST return the DAV header with compliance-class "1" on all OPTIONS responses.

### 注
RFC 4918 全文**没有出现 "Range" 相关的要求**（对 `Range` 一词的检索仅命中无关文本），它把 HTTP 语义授权给 RFC 2616/723x。因此 Range 支持问题完全由 HTTP 规范（RFC 7233/9110）约束，见第 5 节。

---

## 2. `golang.org/x/net/webdav`：确认目录 GET 返回 405

### 结论
**确认（confirmed by source）**。`handleGetHeadPost` 在 `fi.IsDir()` 时**显式** `return http.StatusMethodNotAllowed, nil`。OpenList v4.2.4 内联了该包（`server/webdav`），且额外做了一处修改：**HEAD 目录返回 200**，只有 **GET 目录返回 405**。

### 证据（confirmed by source）

上游 `golang.org/x/net/webdav/webdav.go`（[raw master](https://raw.githubusercontent.com/golang/net/master/webdav/webdav.go)，与 v0.59.0 tag 行号一致）：

```go
// https://github.com/golang/net/blob/v0.59.0/webdav/webdav.go#L214-L232
214: func (h *Handler) handleGetHeadPost(w http.ResponseWriter, r *http.Request) (status int, err error) {
215: 	reqPath, status, err := h.stripPrefix(r.URL.Path)
216: 	if err != nil {
217: 		return status, err
218: 	}
219: 	// TODO: check locks for read-only access??
220: 	ctx := r.Context()
221: 	f, err := h.FileSystem.OpenFile(ctx, reqPath, os.O_RDONLY, 0)
222: 	if err != nil {
223: 		return http.StatusNotFound, err
224: 	}
225: 	defer f.Close()
226: 	fi, err := f.Stat()
227: 	if err != nil {
228: 		return http.StatusNotFound, err
229: 	}
230: 	if fi.IsDir() {
231: 		return http.StatusMethodNotAllowed, nil      // <-- 405，无 Allow 头
232: 	}
233: 	etag, err := findETag(ctx, h.FileSystem, h.LockSystem, reqPath, fi)
...
239: 	http.ServeContent(w, r, reqPath, fi.ModTime(), f)  // <-- 仅文件走 ServeContent
240: 	return 0, nil
241: }
```

分发入口（同一文件 L62–L73）：`GET`/`HEAD`/`POST` 都进 `handleGetHeadPost`。

**OpenList 的实际（内联、已改）版本** — `server/webdav/webdav.go` L222–L253，[OpenList main 分支源码](https://raw.githubusercontent.com/OpenListTeam/OpenList/main/server/webdav/webdav.go)：

```go
246: 	if fi.IsDir() {
247: 		if r.Method == http.MethodHead {
248: 			w.Header().Set("Content-Type", "httpd/unix-directory")
249: 			w.Header().Set("Content-Length", "0")
250: 			return http.StatusOK, nil          // <-- HEAD 目录 = 200（不报错）
251: 		}
252: 		return http.StatusMethodNotAllowed, nil // <-- GET 目录 = 405（本次 bug 的成因）
253: 	}
```

**重要推论（confirmed by source）**：
- **步骤 3 的 `HEAD <collection>` ×3 不会失败**——OpenList 对 HEAD 目录返回 200。只有步骤 2 的 `GET` 会拿到 405。这与"整个连接失败"的表现一致。
- OpenList 的 `CheckAccess` 会先跑，所以 405 之前还有 403 的可能；405 是在通过鉴权+存在性检查之后才返回的。

**OpenList 的 Range 真实实现路径**（同一文件 L274–L290 + [server/common/proxy.go](https://raw.githubusercontent.com/OpenListTeam/OpenList/main/server/common/proxy.go)）：文件 GET 不走 `http.ServeContent`，而走 `common.Proxy` → `net.ServeHTTP`/`stream.GetRangeReaderFromLink`，由驱动层的 `ProxyRange` 能力决定是否流式；不支持时**透传上游响应**。这解释了为什么"文件 + `bytes=0-0` 正确返回 206"：Range 支持是**按存储驱动（per-storage）**的能力，而不是 WebDAV 端点级别的能力。

**版本信息**：OpenList `go.mod`（[link](https://raw.githubusercontent.com/OpenListTeam/OpenList/main/go.mod)）声明 `golang.org/x/net v0.58.0`，内联 `server/webdav`。

---

## 3. 其他真实 WebDAV 服务器：目录 GET 的返回

### 结论
**没有统一行为**。至少出现四类：**405**（Go `x/net/webdav` 系如 OpenList/Alist；**Nextcloud 亦为 405**）、**501**（纯 SabreDAV 库默认，除非启用 Browser 插件）、**301→403/索引页**（nginx、Apache 默认静态处理）、**200 HTML 列表**（启用目录浏览/插件时）。

**这本身就是"不应探测 collection"的最强论据**：同一个探测在 OpenList 得到 405、在纯 SabreDAV 得到 501、在 nginx 得到 301/403——**无论客户端特判哪一个具体码，都会在另一类服务器上失败**。而且 [nextcloud/server#48602](https://github.com/nextcloud/server/issues/48602) 显示 **gvfs 已经踩过完全相同的坑**（对 collection 做能力探测 → 405 → 卡死），Nextcloud 维护者的结论是"这是客户端 bug，客户端应把 405 当作错误并中止其智能探测"。

| 服务器 | 目录 GET 结果 | 置信度 |
|---|---|---|
| Go `x/net/webdav`（OpenList/Alist） | **405** | ✅ 源码确认 |
| **Nextcloud**（`remote.php/dav/files/`） | **405** | ✅ 实测日志（上游 issue） |
| SabreDAV（库默认，无插件） | **501**（Browser 插件启用时 **200** HTML 列表） | ✅ 源码确认 |
| nginx `ngx_http_dav_module` | **301 / 403 / 200**（DAV 模块不管 GET） | ✅ 源码+官方文档确认 |
| Apache `mod_dav` | **301 / 403 / 200**（默认走静态/`mod_autoindex`） | ✅ 源码确认 |
| IIS WebDAV | **405**（WebDAV 模块拦截时，Allow 列 WebDAV 方法）/ 否则 403 | ⚠️ 推断 |
| ownCloud（ocis 新版） | 未确定（另见 501 报告） | ❌ could not determine |
| Synology WebDAV Server | 未确定 | ❌ could not determine |
| 坚果云 jianguoyun | 未确定 | ❌ could not determine |
| 115 网盘 WebDAV | 未确定 | ❌ could not determine |

### 逐项证据

#### 3.1 OpenList / Alist（Go `x/net/webdav`）→ 405
见第 2 节。**confirmed by source**。

#### 3.2 Nextcloud → **405（实测）**；SabreDAV 库默认 → **501**
把这两者分开看非常重要，因为**它们不是同一个状态码**。

**(a) Nextcloud：405 —— 有实测证据。** [nextcloud/server#48602 "WebDav client `gvfs` get stuck at files/ in WebDav root with 405 Not Allowed"](https://github.com/nextcloud/server/issues/48602) 的报告者明确写道：

> Unfortunately there is no way to get to the user's home at files/user/ again, because **if You navigate into files/, the server will respond with 405 "Not Allowed"**.

并在后续评论中给出**逐条 HTTP 访问日志**，以及对 `/remote.php/dav/files/` 的 `GET` 得到 405；社区给出的 workaround 正是"用 nginx 把这个路径的 405 改写成 404 以骗过客户端的能力探测"：

> Based on observations, I found a solution to hotfix this in my webserver config by
> ```
> location = /remote.php/dav/files/ {
>     return 404;
> }
> ```
> This breaks the service discovery gvfs automatically does, by **overriding the 405 "Not Allowed" with 404 "Not Found"**.

**这几乎是 RCH 当前 bug 的镜像**：gvfs 也对 collection 做了"智能探测"，Nextcloud 回 405，客户端因此卡死。Nextcloud 维护者的态度是"这属于客户端 bug，客户端应当把 405 当错误处理":

> So as said this seems like a gvfs bug, **they should consider the 405 also an error to abort their "smart" detection**.

> 注：Nextcloud 具体在何处抛出 405 **未能定位到源码行**（在 `apps/dav/lib/**` 与 `remote.php` 中检索 `MethodNotAllowed` 均未命中）。**观察到的状态码本身证据充分、可引用；实现机制标注为 inferred。**

**(b) SabreDAV 库默认（无 Browser 插件）：501。** `lib/DAV/CorePlugin.php` 的 `httpGet` 对非 `IFile` 节点**直接 return（不设置响应）**（[sabre/dav master](https://raw.githubusercontent.com/sabre-io/dav/master/lib/DAV/CorePlugin.php) L69–L76）：

```php
69: public function httpGet(RequestInterface $request, ResponseInterface $response)
70: {
71:     $path = $request->getPath();
72:     $node = $this->server->tree->getNodeForPath($path);
73:
74:     if (!$node instanceof IFile) {
75:         return;          // collection：不处理，交给下一个 handler
76:     }
```

`lib/DAV/Server.php` `invokeMethod` 随后在**无插件认领**时抛异常（[Server.php](https://raw.githubusercontent.com/sabre-io/dav/master/lib/DAV/Server.php) L472–L480）：

```php
472: if ($this->emit('method:'.$method, [$request, $response])) {
473:     $exMessage = 'There was no plugin in the system that was willing to handle this '.$method.' method.';
474:     if ('GET' === $method) {
475:         $exMessage .= ' Enable the Browser plugin to get a better result here.';
476:     }
477:     // Unsupported method
478:     throw new Exception\NotImplemented($exMessage);
479: }
```

而 `Sabre\DAV\Exception\NotImplemented::getHTTPCode()` 返回 **501**（[NotImplemented.php](https://raw.githubusercontent.com/sabre-io/dav/master/lib/DAV/Exception/NotImplemented.php)）：

```php
public function getHTTPCode()
{
    return 501;
}
```

**启用 Browser 插件则变成 200**：`lib/DAV/Browser/Plugin.php` 以 priority 200 注册 `method:GET`（高于 CorePlugin），其文档注释即 *"This method intercepts GET requests to collections and returns the html."*（[Browser/Plugin.php](https://raw.githubusercontent.com/sabre-io/dav/master/lib/DAV/Browser/Plugin.php) L77–L102）。

> ⚠️ **对本 bug 的直接意义**：RCH 目前只特判了 `405`。**Nextcloud 是 405（能命中，但仍被当致命错误）**，而**纯 SabreDAV 后端是 501**；RCH 把 **501 归入 `500..=599` 的"暂时性网络错误"**（`app/rust/src/remote_scan/adapter.rs` L70–L78），错误信息具有误导性。修 405 时**必须一并覆盖 501**，否则只修好 OpenList 一家。

#### 3.3 nginx `ngx_http_dav_module` → DAV 模块**不处理 GET**
官方文档明确列出模块只处理 5 个方法（[nginx docs](https://nginx.org/en/docs/http/ngx_http_dav_module.html)）：

> The module processes HTTP and WebDAV methods **PUT, DELETE, MKCOL, COPY, and MOVE**. … **WebDAV clients that require additional WebDAV methods to operate will not work with this module.**

源码印证：`ngx_http_dav_handler` 只 switch `PUT/DELETE/MKCOL/COPY/MOVE`，**其余一律 `return NGX_DECLINED`**（[ngx_http_dav_module.c](https://raw.githubusercontent.com/nginx/nginx/master/src/http/modules/ngx_http_dav_module.c) L146–L205）。所以 GET 目录落到静态/索引模块：

- `ngx_http_static_module`：目录且 URI 未以 `/` 结尾 → **301**（`NGX_HTTP_MOVED_PERMANENTLY`，[ngx_http_static_module.c](https://raw.githubusercontent.com/nginx/nginx/master/src/http/modules/ngx_http_static_module.c) L148–L203）；非 GET/HEAD/POST → `NGX_HTTP_NOT_ALLOWED`（L63–L64）。
- `ngx_http_autoindex_module`：`autoindex off`（默认）时 **`return NGX_DECLINED`**（[ngx_http_autoindex_module.c](https://raw.githubusercontent.com/nginx/nginx/master/src/http/modules/ngx_http_autoindex_module.c) L177–L178），最终由 nginx 默认 content handler 产出 **403**。开启 `autoindex on` 则 **200 HTML 列表**。

**结果：nginx 上"探测 collection"通常拿到 301 或 403，不是 405。** 若 RCH 把"非 206"一律当失败，nginx 同样会失败（只是错误码不同）。

#### 3.4 Apache `mod_dav` → 默认走静态处理（**301 / 403 / 200**）
`dav_handler` 仅在 `r->handler == "DAV"` 时才接管（[mod_dav.c](https://raw.githubusercontent.com/apache/httpd/trunk/modules/dav/main/mod_dav.c) L5152–L5164）：

```c
5163: if (strcmp(r->handler, DAV_HANDLER_NAME) != 0)
5164:     return DECLINED;
```

即便接管，`dav_method_get` 的注释说明它**只处理"对 Apache 不可见"的资源**（L1005–L1008），目录依旧交给下层：

```c
1005: /* This method should only be called when the resource is not
1006:  * visible to Apache. We will fetch the resource from the repository,
1007:  * then create a subrequest for Apache to handle.
1008:  */
```

后续目录处理：`mod_dir` 做 trailing-slash 重定向或找 `DirectoryIndex`；找不到索引且禁止目录列表时 `mod_autoindex` 返回 **403 Forbidden**（[mod_autoindex.c](https://raw.githubusercontent.com/apache/httpd/trunk/modules/generators/mod_autoindex.c) L2343–L2349）：

```c
2343: ap_log_rerror(..., "Cannot serve directory %s: No matching DirectoryIndex (%s) found, and "
2344:               "server-generated directory index forbidden by Options directive", ...);
2349: return d->not_found ? HTTP_NOT_FOUND : HTTP_FORBIDDEN;
```

`dav_method_get` 本身只对**不存在的资源**返回 404（L1020–L1023），**不返回 405**。官方 mod_dav 文档（[httpd docs](https://httpd.apache.org/docs/trunk/en/mod/mod_dav.html)）亦未定义 collection 上 GET 的 405 语义。

#### 3.5 IIS WebDAV → 未完全确定
**inferred**：IIS 的 WebDAV 模块作为 handler 接管 DAV 请求时，会以 `405 Method Not Allowed` 拒绝它未开放的方法（`Allow` 头列出 `OPTIONS, PROPFIND, PROPPATCH, LOCK, UNLOCK, ...`），这在大量 mod_dav 系实现上一致；若 WebDAV 模块未启用于该路径，则由 IIS 静态/目录处理给出 403/404。
**未能找到**一份明确写"IIS 对 GET 目录返回 X"的权威文档。列为 **could not determine**（偏 405）。

#### 3.6 Synology WebDAV Server → **could not determine**
**未能确定**。检索到的 [CherryHQ/cherry-studio#2679](https://github.com/CherryHQ/cherry-studio/issues/2679) 表面上提到"群晖 WebDAV 备份报 405 Method Not Allowed"，但逐字核对 issue 正文后确认：**这是路径配置错误，不是服务端行为**——维护者回复：

> 这不是 app 的问题，是群晖 webdav 的设置问题：**路径写错了，volume1 是群晖存储池1，内部使用的，不应该出现在路径上**。直接写你打开权限了的共享文件夹路径，例如原来的 `/volume1/cherry-studio` 改为 `/cherry-studio` 即可

因此**该 issue 不能作为"Synology 对目录 GET 返回 405"的证据**。Synology 的 WebDAV Server 包在 DSM 内部实现未公开，未找到可靠证据。

#### 3.7 坚果云 / 115 网盘 → **could not determine**
**未能确定**。两者均为闭源服务，未找到针对"目录 GET 状态码"的权威说明或可复现报告。**不做状态码断言**（早期基于生态的"推测 501"已被 3.2 的 Nextcloud 实测反例削弱——同为 SabreDAV 生态的 Nextcloud 实际是 405，因此生态推断不可靠）。

---

## 4. 成熟 WebDAV 客户端/库如何判定 Range 支持

### 结论
**没有发现任何成熟客户端对 collection 做 Range 探测。** 主流做法是：**直接对文件发带 `Range` 的 GET，然后校验响应**（rclone），**或者根本不做任何探测/不支持 Range**（go-webdav、webdavclient3）。**没有任何实现把"探测 collection 得到 405/501"当作致命错误。**

### 4.1 rclone `webdav` backend → **按文件探测，且以响应校验代替能力探测**
✅ **confirmed by source**。核心在 `Object.Open`（[rclone backend/webdav/webdav.go](https://raw.githubusercontent.com/rclone/rclone/master/backend/webdav/webdav.go) L1556–L1588）：

```go
1556: func (o *Object) Open(ctx context.Context, options ...fs.OpenOption) (in io.ReadCloser, err error) {
1557: 	var resp *http.Response
1558: 	fs.FixRangeOption(options, o.size)
1559: 	opts := rest.Opts{
1560: 		Method:  "GET",
1561: 		Path:    o.filePath(),        // <-- 单文件路径，绝不是 collection
1562: 		Options: options,             // <-- Range 作为普通 open option 下发
1563: 		ExtraHeaders: map[string]string{
1564: 			"Depth": "0",
1565: 		},
...
1571: 			err = rest.CheckContentRange(resp, options, o.size)
...
1576: 				if errors.Is(err, fs.ErrorRangeIgnored) {
1577: 					return false, err
1578: 				}
```

要点：
- **`Path: o.filePath()`**——Range 永远绑定到**对象（文件）**，不存在对目录的探测。
- 没有单独的"探测 Range 支持"阶段；**直接读，再用 `rest.CheckContentRange` 校验 `Content-Range`**。
- 服务器忽略 Range 时通过哨兵错误 `fs.ErrorRangeIgnored = errors.New("server ignored requested range")`（[fs/fs.go](https://raw.githubusercontent.com/rclone/rclone/master/fs/fs.go)）显式表达，**是正常可处理路径，不是致命失败**。
- 未使用 `Accept-Ranges` 做能力判定。`Vendor` 选项（`nextcloud`/`owncloud`/`sharepoint`/`other`，[L79–L103](https://raw.githubusercontent.com/rclone/rclone/master/backend/webdav/webdav.go)）只用于调别的 quirk（分块上传、Depth 行为等），**与 Range 无关**。这与 RCH "按需探测"的设计目标最接近，值得直接借鉴（Adopt→Adapt）。

### 4.2 `go-webdav`（emersion）→ **完全不支持 Range**
✅ **confirmed by source**。`Client.Open` 只是一次普通 GET 并返回 body，没有 Range 选项、没有 `Accept-Ranges` 检查（[go-webdav client.go](https://raw.githubusercontent.com/emersion/go-webdav/master/client.go) L142）：

```go
142: func (c *Client) Open(ctx context.Context, name string) (io.ReadCloser, error) {
```

整个 `client.go` 中 **`Range` / `Accept-Ranges` / `Content-Range` 零命中**。

### 4.3 Python `webdavclient3` → **完全不支持 Range**
✅ **confirmed by source**。`webdav3/client.py`（v3.14.7，上游 [ezhov-evgeny/webdav-client-python-3](https://github.com/ezhov-evgeny/webdav-client-python-3)）中 **`Range` / `Accept-Ranges` / `Content-Range` 零命中**——既不探测也不使用。

### 4.4 cadaver / libneon / Cyberduck / Windows WebClient / gvfs / davfs2
❌ **could not determine（本轮未取得源码级证据）**。
已知的定位事实：**cadaver 基于 neon**（[cadaver 官网](https://notroj.github.io/cadaver/)），libneon 是 Subversion 的 HTTP/DAV 库，因此 Range 行为受 neon 的 `ne_ranges`/HTTP 层控制，而非 WebDAV 层。但这些实现的"是否探测 collection"**未取得可引用证据**，不做断言。**注意：即便它们探测，目标也必然是非 collection 资源**——因为按第 1 节，collection 上的 GET 结果本身不可依赖。

### 4.5 是否有客户端"对目录探测 Range"？
❌ **未发现任何此类实现**。检索 `range probe` + WebDAV 405 相关 issue，**未找到**任何成熟客户端把 Range 能力探测指向 collection 的记录。RCH 的做法在这一点上是孤例。

---

## 5. Range 支持应当如何正确判定

### 结论
**推荐：不要做独立的"能力探测"，而是"直接尝试 + 校验 + 优雅回退"**（rclone 模式）。若 RCH 因架构原因必须保留连接期探测，则**必须**：
1. **探测目标改为真实文件**（不是 collection）；
2. **把非 206 视为"不支持/未知"，而不是"连接失败"**——绝不让探测否决连接；
3. `Accept-Ranges: bytes` 只能作为**正向提示**，不能作为必要条件（规范明示客户端可以不等它）；
4. 一次性校验 `Content-Range` 与 `206` 的一致性（RCH 已在 `classify_range_probe_response` 做了这件事，可保留）。

### 证据（confirmed by spec）

**RFC 7233 §2.3 `Accept-Ranges`**（[RFC 7233](https://www.rfc-editor.org/rfc/rfc7233.txt)）：

> The "Accept-Ranges" header field allows a server to indicate that it supports range requests **for the target resource**. … An origin server that supports byte-range requests for a given target resource **MAY** send `Accept-Ranges: bytes` … **A client MAY generate range requests without having received this header field for the resource involved.** … A server that does not support any kind of range request for the target resource MAY send `Accept-Ranges: none`.

**RFC 9110 §14.3 `Accept-Ranges`**（更新版措辞，[RFC 9110](https://www.rfc-editor.org/rfc/rfc9110.txt)）：

> A client **MAY** generate range requests **regardless of having received an Accept-Ranges field**. The information only provides advice for the sake of improving performance and reducing unnecessary network transfers.
>
> Conversely, **a client MUST NOT assume that receiving an Accept-Ranges field means that future range requests will return partial responses.** The content might change, the server might only support range requests at certain times or under certain conditions, or a different intermediary might process the next request.

**RFC 7233 §3.1 `Range`**（[RFC 7233](https://www.rfc-editor.org/rfc/rfc7233.txt)）：

> The "Range" header field on a **GET request** modifies the method semantics to request transfer of only one or more subranges of the selected representation data…
>
> **A server MAY ignore the Range header field.** However, origin servers and intermediate caches **ought to support byte ranges when possible**… **A server MUST ignore a Range header field received with a request method other than GET.**

### 由此得出的判定规则（本简报的建议）

| 手段 | 可用性 | 说明 |
|---|---|---|
| 对**真实文件**发 `Range: bytes=0-0`，期望 206 + `Content-Range` | ✅ **首选** | 唯一能真正证明"该资源支持 Range"的方法；RCH 已具备该路径（`range_probe_checked`），只需换目标 |
| 读 `Accept-Ranges: bytes` | ✅ **仅作正向提示** | 规范明确：可为空、可缺失，客户端**不得**依赖。不可作为 gating 条件 |
| 读 `Content-Range` | ✅ **响应校验** | 用于确认 206 的语义正确（当前代码已校验，很好） |
| 读 `DAV:` 头 | ❌ **无关** | 只表示 compliance class，不表示 Range（RFC 4918 §10.1） |
| `Content-Length` | ⚠️ 辅助 | 可推知大小，但**不能**推知 Range 支持 |
| 直接尝试 ranged read，失败回退整包 | ✅ **最稳健** | rclone 模式；把"忽略 Range"当作一等公民结果而非错误 |

### 对 RCH 的直接建议（不改代码，仅结论）
- **致命性错误**：`check_and_probe` 中 `.map_err(|error| anyhow!(error.to_string()))?`（`app/rust/src/source/webdav.rs` L254–L257）把探测错误升级为连接失败。**探测失败应当只影响 `capability.range_supported`，不应否决连接。**
- **探测目标错误**：`check_and_probe` 用 `root`（= collection）作为探测路径（同文件 L255）。应**列一次目录、挑一个真实文件**探测；无文件时直接判定"未知/不支持"，并退化为整包下载。
- **必须一并放宽 405 之外的码**：`adapter.rs` L64–L69 只特判 405；**501（SabreDAV/Nextcloud 系）会被归入 `500..=599` 暂时性网络错误**。建议把 **405/501/403/404** 以及"200（忽略 Range）"统一归类为 **`supported = false`**（能力问题），只有网络层错误/401 才是真正的失败。
- 保留现有 `206` + `Content-Range` 严格校验逻辑（`adapter.rs` L82–L114），它对"假 206"的防御是正确的。

---

## 6. 边界情况：客户端有正当理由 GET collection 吗？有 DAV 属性广告 Range 吗？

### 结论
1. **有，但只在"人可读浏览"场景**——RFC 4918 §9.4 明确允许（"human-readable view of the contents of the collection"）。浏览器/管理界面 GET 目录是合法的；但对**机器自动化客户端**（如 RCH 的 Range 探测）没有任何正当理由——`PROPFIND` 才是列目录的正确手段（RCH 步骤 1 用 `PROPFIND Depth: 0` 是正确的）。
2. **`Accept-Ranges` 不出现在 PROPFIND 响应中，也不是 DAV 属性。** **不存在**任何标准 DAV 属性广告 Range 支持。

### 证据（confirmed by spec）
- **RFC 4918 §15 定义了且仅定义了 10 个 DAV 属性**：`creationdate`(15.1)、`displayname`(15.2)、`getcontentlanguage`(15.3)、`getcontentlength`(15.4)、`getcontenttype`(15.5)、`getetag`(15.6)、`getlastmodified`(15.7)、`lockdiscovery`(15.8)、`resourcetype`(15.9)、`supportedlock`(15.10)。**其中没有任何 Range/`supported-*` 属性**。RFC 4918 全文对 `Range` 一词的检索不命中任何规范条款（仅命中无关文本）。
- `DAV:getcontentlength` 的定义只与 **GET 的 `Content-Length`** 绑定（§15.4），与 Range 无关。
- `Accept-Ranges` 是 **HTTP 响应头**，其语义绑定"GET 所选表示"（RFC 7233 §2.3 / RFC 9110 §14.3）。**PROPFIND 响应没有"所选表示"**，因此 `Accept-Ranges` 出现在 PROPFIND 响应上属实现副作用，**规范上不可依赖**。
- `DAV:` 响应头（RFC 4918 §10.1）**只承载 compliance class**（`1`/`2`/`3` 或 Coded-URL 扩展），且**只在 OPTIONS 响应上被要求**。规范未定义 `DAV:` 中任何与 Range 有关的 token。§18 的 Class 1/2/3 义务里也没有 Range。
- 存在的 `{DAV:}supported-report-set`（RFC 3253）只广告 **REPORT** 类型，与 Range 无关。

### 另有一处"collection 上必须支持 GET"的例外（供对照）
RFC 4918 §9.10.4 要求 LOCK 产生的**空资源**必须能 GET：

> A server MUST respond successfully to a GET request to an empty resource, either by using a 204 No Content response, or by using 200 OK with a Content-Length header indicating zero length

注意这是 **empty resource（非 collection）**，不适用于目录。

### 对 OpenList 的具体验证
✅ **confirmed by source**：OpenList 的 WebDAV 实现**未在任何地方**设置 `Accept-Ranges`（`server/webdav/webdav.go` 全文 `Accept-Ranges` 零命中；只有文件路径经 `common.Proxy` → `net.ServeHTTP`）。因此**无法通过任何响应头判断 OpenList 的 Range 能力**——唯一可靠办法就是对真实文件发一次 ranged GET。

---

## 附：证据等级汇总

| 结论 | 等级 |
|---|---|
| RFC 4918 §9.4 允许 collection GET 返回"something else altogether"；405 合法 | ✅ spec |
| RFC 7231/9110 405/501 定义 | ✅ spec |
| RFC 7233 §2.3 / RFC 9110 §14.3 `Accept-Ranges` 是 MAY、非必需、不可依赖 | ✅ spec |
| `golang.org/x/net/webdav` `handleGetHeadPost` 对目录返回 405 | ✅ 源码 |
| OpenList 内联该包，**GET 目录 405 / HEAD 目录 200** | ✅ 源码 |
| OpenList 文件 Range 走 `common.Proxy`，按 storage 驱动能力 | ✅ 源码 |
| SabreDAV（库默认）对 collection GET 抛 `NotImplemented` → **501** | ✅ 源码 |
| SabreDAV Browser 插件启用时 collection GET → 200 HTML | ✅ 源码 |
| **Nextcloud 对 `remote.php/dav/files/` 的 GET → 405** | ✅ 上游 issue 实测日志 |
| Nextcloud 抛 405 的具体代码位置 | ❌ 未定位 |
| nginx DAV 模块不处理 GET，目录 GET → 301/403/200 | ✅ 源码 + 官方文档 |
| Apache mod_dav 默认不接管目录 GET，→ 301/403/200 | ✅ 源码 |
| IIS 对目录 GET 返回 405 | ⚠️ 推断 |
| Synology / 坚果云 / 115 / ownCloud(ocis) 对目录 GET 的行为 | ❌ 未确定（未找到可靠证据） |
| rclone 按文件 Range + `ErrorRangeIgnored` 回退，不探测 | ✅ 源码 |
| go-webdav / webdavclient3 完全不支持 Range | ✅ 源码 |
| cadaver / libneon / Cyberduck / Windows WebClient / gvfs / davfs2 | ❌ 未取得源码级证据 |
| 无任何成熟客户端对 collection 做 Range 探测 | ⚠️ 未发现反例（非穷尽检索） |

## 参考链接
- RFC 4918 (WebDAV): https://www.rfc-editor.org/rfc/rfc4918.txt
- RFC 7233 (HTTP Range Requests): https://www.rfc-editor.org/rfc/rfc7233.txt
- RFC 7231 (HTTP Semantics): https://www.rfc-editor.org/rfc/rfc7231.txt
- RFC 9110 (HTTP Semantics, current): https://www.rfc-editor.org/rfc/rfc9110.txt
- golang.org/x/net webdav: https://github.com/golang/net/blob/v0.59.0/webdav/webdav.go
- OpenList WebDAV 路由: https://raw.githubusercontent.com/OpenListTeam/OpenList/main/server/webdav.go
- OpenList WebDAV handler: https://raw.githubusercontent.com/OpenListTeam/OpenList/main/server/webdav/webdav.go
- OpenList proxy/Range: https://raw.githubusercontent.com/OpenListTeam/OpenList/main/server/common/proxy.go
- SabreDAV CorePlugin: https://raw.githubusercontent.com/sabre-io/dav/master/lib/DAV/CorePlugin.php
- SabreDAV Server: https://raw.githubusercontent.com/sabre-io/dav/master/lib/DAV/Server.php
- SabreDAV NotImplemented: https://raw.githubusercontent.com/sabre-io/dav/master/lib/DAV/Exception/NotImplemented.php
- SabreDAV Browser Plugin: https://raw.githubusercontent.com/sabre-io/dav/master/lib/DAV/Browser/Plugin.php
- nginx dav module docs: https://nginx.org/en/docs/http/ngx_http_dav_module.html
- nginx dav source: https://raw.githubusercontent.com/nginx/nginx/master/src/http/modules/ngx_http_dav_module.c
- nginx static source: https://raw.githubusercontent.com/nginx/nginx/master/src/http/modules/ngx_http_static_module.c
- nginx autoindex source: https://raw.githubusercontent.com/nginx/nginx/master/src/http/modules/ngx_http_autoindex_module.c
- Apache mod_dav docs: https://httpd.apache.org/docs/trunk/en/mod/mod_dav.html
- Apache mod_dav source: https://raw.githubusercontent.com/apache/httpd/trunk/modules/dav/main/mod_dav.c
- Apache mod_autoindex source: https://raw.githubusercontent.com/apache/httpd/trunk/modules/generators/mod_autoindex.c
- rclone webdav backend: https://raw.githubusercontent.com/rclone/rclone/master/backend/webdav/webdav.go
- go-webdav: https://raw.githubusercontent.com/emersion/go-webdav/master/client.go
- webdavclient3: https://github.com/ezhov-evgeny/webdav-client-python-3
- CherryHQ/cherry-studio#2679（**已排除**：群晖 405 系路径配置错误）: https://github.com/CherryHQ/cherry-studio/issues/2679
- nextcloud/server#48602（**关键**：Nextcloud 对 collection GET 返回 405，gvfs 同样被卡死）: https://github.com/nextcloud/server/issues/48602
- GNOME gvfs 对应缺陷: https://gitlab.gnome.org/GNOME/gvfs/-/issues/769
