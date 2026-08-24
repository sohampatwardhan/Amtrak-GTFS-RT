"""Tests for the atomic, churn-free snapshot store (task 3.2, Requirements 2.2/2.3/8.2)."""

from fetcher.store import SNAPSHOT_NAME, SnapshotStore


def test_read_none_before_first_write(tmp_path):
    store = SnapshotStore(tmp_path)
    assert store.read() is None


def test_update_then_read_roundtrip(tmp_path):
    store = SnapshotStore(tmp_path)
    assert store.update("<html>na-service-alert</html>") is True
    snapshot = store.read()
    assert snapshot is not None
    data, last_success = snapshot
    assert data == b"<html>na-service-alert</html>"
    assert last_success > 0


def test_unchanged_content_is_not_rewritten(tmp_path):
    store = SnapshotStore(tmp_path)
    store.update("same")
    inode_before = store.path.stat().st_ino
    changed = store.update("same")
    assert changed is False
    # os.replace would allocate a new inode; a no-rewrite keeps the same file (only mtime bumped).
    assert store.path.stat().st_ino == inode_before
    assert store.path.read_bytes() == b"same"


def test_changed_content_is_rewritten(tmp_path):
    store = SnapshotStore(tmp_path)
    store.update("v1")
    assert store.update("v2") is True
    assert store.path.read_bytes() == b"v2"


def test_write_leaves_no_temp_files(tmp_path):
    store = SnapshotStore(tmp_path)
    store.update("payload")
    leftovers = [p.name for p in tmp_path.iterdir() if p.name != SNAPSHOT_NAME]
    assert leftovers == []
