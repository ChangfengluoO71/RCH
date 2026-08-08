//! PDF 原生库目录配置（Android 打包 libpdfium.so 后由 Dart 侧传入 nativeLibraryDir）。

use crate::document::pdf;

/// 设置 pdfium 原生库所在目录（Android：`ApplicationInfo.nativeLibraryDir`）。
/// 调用后打开 PDF 时优先从该目录加载 `libpdfium.so`；Windows 桌面端无需调用。
pub fn set_native_lib_dir(dir: String) {
    pdf::set_native_lib_dir(dir);
}
