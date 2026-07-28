// LibraryRepository: 统一数据访问入口（ADR-016）。
//
// TagRepository + LibraryStore 的统一 facade。
// UI 层通过 LibraryRepository 访问所有数据，不直接依赖具体存储实现。

library;

export '../store/library_store.dart';
export 'tag_repository.dart';
