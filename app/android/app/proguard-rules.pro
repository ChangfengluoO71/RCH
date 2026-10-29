# WorkManager creates its Room database implementation through reflection.
# Preserve the generated zero-argument constructor under R8 full mode.
-keep class androidx.work.impl.WorkDatabase_Impl { <init>(); }
