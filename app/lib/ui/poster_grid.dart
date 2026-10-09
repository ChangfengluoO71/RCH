import 'package:app/store/library_store.dart';
import 'package:app/ui/common.dart';
import 'package:flutter/material.dart';

/// Keeps desktop's max-width poster layout while letting compact layouts use a
/// consistent, user-selected fixed number of columns.
SliverGridDelegate comicPosterGridDelegate(
  BuildContext context, {
  required double maxCrossAxisExtent,
  required double childAspectRatio,
  required double crossAxisSpacing,
  required double mainAxisSpacing,
}) {
  if (!isCompact(context)) {
    return SliverGridDelegateWithMaxCrossAxisExtent(
      maxCrossAxisExtent: maxCrossAxisExtent,
      childAspectRatio: childAspectRatio,
      crossAxisSpacing: crossAxisSpacing,
      mainAxisSpacing: mainAxisSpacing,
    );
  }

  final columns = LibraryStore.instance.settings.mobilePosterColumns
      .clamp(2, 4)
      .toInt();
  return SliverGridDelegateWithFixedCrossAxisCount(
    crossAxisCount: columns,
    childAspectRatio: childAspectRatio,
    crossAxisSpacing: crossAxisSpacing,
    mainAxisSpacing: mainAxisSpacing,
  );
}
