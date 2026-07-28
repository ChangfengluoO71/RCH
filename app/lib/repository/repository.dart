// LibraryRepository: 统一数据访问入口（ADR-016）。
//
// BookRepository + RecordRepository + TagRepository + LibraryStore 的统一 facade。
// UI 层通过 LibraryRepository 访问所有数据，不直接依赖具体存储实现。

library;

export '../store/library_store.dart';
export 'book_repository.dart';
export 'record_repository.dart';
export 'tag_repository.dart';
