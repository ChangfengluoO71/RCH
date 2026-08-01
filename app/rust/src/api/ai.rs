//! 暴露给 Dart 的 AI 超分 API。

/// 对单张页面图片执行 AI 超分（默认 2x，scale 参数传给 CLI 的 -s）。
/// scale 参数用于传递给 CLI 的 -s 参数。
pub fn super_resolve(page_bytes: Vec<u8>, scale: u32) -> anyhow::Result<Vec<u8>> {
    crate::ai::super_resolve(&page_bytes, scale)
}

/// 查找已缓存的 AI 超分结果。
/// 若缓存存在，返回超分后的 JPEG；否则返回 null。
pub fn lookup_cache(page_bytes: Vec<u8>, scale: u32) -> Option<Vec<u8>> {
    crate::ai::lookup_cache(&page_bytes, scale)
}

/// 批量 AI 超分 — 一次 CLI 调用处理所有页。
pub fn super_resolve_batch(pages: Vec<Vec<u8>>, scale: u32) -> anyhow::Result<Vec<Vec<u8>>> {
    crate::ai::super_resolve_batch(&pages, scale)
}

/// 删除某页的 AI 超分缓存（取消整本超分时按页清理，不影响其他书）。
pub fn delete_ai_cache_for_page(page_bytes: Vec<u8>, scale: u32) {
    crate::ai::delete_ai_cache_for_page(&page_bytes, scale);
}
