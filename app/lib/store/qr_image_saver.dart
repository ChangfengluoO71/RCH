import 'dart:typed_data';

import 'package:gal/gal.dart';

/// Saves a generated QR image in a user-visible gallery album.
///
/// The bytes are kept in memory only; [Gal] writes them through the native
/// media store on Android and the equivalent gallery API on supported
/// platforms.
Future<void> saveQrImageToGallery(Uint8List bytes) async {
  if (bytes.isEmpty) {
    throw const FormatException('QR image is empty');
  }
  final name = 'rch-115-qr-${DateTime.now().millisecondsSinceEpoch}';
  await Gal.putImageBytes(bytes, album: 'RCH', name: name);
}
