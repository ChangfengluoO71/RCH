# Trellis State Reconciliation

## Goal

Reconcile stale Trellis task state against repository implementation, release history, task acceptance evidence, and current actionable scope without changing product code.

## Acceptance Criteria

- [x] Classify all 26 previously unarchived tasks.
- [x] Archive completed or superseded historical tasks with evidence.
- [x] Preserve genuinely actionable tasks.
- [x] Preserve parent-child links and let Trellis compute progress from active children.
- [x] Verify the resulting active task tree through CWapi/Trellis.
- [x] Make no product-source mutation.
