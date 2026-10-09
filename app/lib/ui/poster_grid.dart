import 'package:app/store/library_store.dart';
import 'package:app/ui/common.dart';
import 'package:flutter/material.dart';

/// Keeps desktop's max-width poster layout while letting compact layouts use a
/// consistent, user-selected fixed number of columns.
SliverGridDelegate comicPosterGridDelegate(
  BuildContext context, {
  required double maxCrossAxisExtent,
  required double childAspectRatio,
  required double mobileCoverAspectRatio,
  required double crossAxisSpacing,
  required double mainAxisSpacing,
  required double mobileHorizontalPadding,
  double mobileCaptionExtent = 58,
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
  final availableWidth =
      MediaQuery.sizeOf(context).width -
      MediaQuery.paddingOf(context).horizontal;
  final tileWidth =
      (availableWidth -
          mobileHorizontalPadding -
          crossAxisSpacing * (columns - 1)) /
      columns;
  return SliverGridDelegateWithFixedCrossAxisCount(
    crossAxisCount: columns,
    // Keep the cover at its intended aspect ratio and allocate caption space
    // below it. A ratio for the entire card would shrink covers as captions
    // take up a larger share at 3–4 columns.
    mainAxisExtent: tileWidth / mobileCoverAspectRatio + mobileCaptionExtent,
    crossAxisSpacing: crossAxisSpacing,
    mainAxisSpacing: mainAxisSpacing,
  );
}
