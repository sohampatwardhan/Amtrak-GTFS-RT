"""Snapshot HTTP routes, including an empty successful poll."""

import json

from fetcher.server import SOURCE_TEMPLATE, route
from fetcher.store import TrackStore


def test_health_and_unknown_paths():
    store = TrackStore()
    status, content_type, body = route(store, "/tracks.json", 300, 7200, "/healthz", 1_000)
    assert (status, content_type, body) == (200, "text/plain", b"ok")
    status, _, body = route(store, "/tracks.json", 300, 7200, "/other", 1_000)
    assert status == 404
    assert body == b"not found"


def test_missing_and_stale_snapshots_are_503():
    store = TrackStore()
    status, _, body = route(store, "/tracks.json", 300, 7200, "/tracks.json", 1_000)
    assert status == 503
    assert body == b"no snapshot yet"
    store.mark_success(1_000)
    status, content_type, body = route(store, "/tracks.json", 300, 7200, "/tracks.json", 1_100)
    assert status == 200
    assert content_type == "application/json"
    payload = json.loads(body)
    assert payload["source"] == SOURCE_TEMPLATE
    assert payload["documents"] == []
    assert payload["fetched_at"] == "1970-01-01T00:16:40Z"
    status, _, body = route(store, "/tracks.json", 300, 7200, "/tracks.json", 1_000 + 301)
    assert status == 503
    assert body == b"snapshot stale"


def test_a_stored_document_is_served_until_its_ttl():
    store = TrackStore()
    store.put("167", "2026-10-17", {"data": [{"travelService": {"number": "167"}}]}, 1_000)
    store.mark_success(1_000)
    _, _, body = route(store, "/tracks.json", 300, 50, "/tracks.json", 1_040)
    assert json.loads(body)["documents"][0]["data"][0]["travelService"]["number"] == "167"
    _, _, body = route(store, "/tracks.json", 300, 50, "/tracks.json", 1_060)
    assert json.loads(body)["documents"] == []


def test_documents_carry_their_fetch_time_and_tracked_stations():
    store = TrackStore()
    document = {
        "data": [
            {
                "travelService": {"number": "167"},
                "stops": [
                    {"station": {"code": "PVD"}, "departure": {"track": {"number": "1"}}},
                    {"station": {"code": "kin"}, "arrival": {"track": {"number": 2}}},
                    {"station": {"code": "BOS"}, "departure": {"track": {"number": ""}}},
                ],
            }
        ]
    }
    store.put("167", "2026-10-17", document, 1_000)
    store.mark_success(1_000)
    _, _, body = route(store, "/tracks.json", 300, 7200, "/tracks.json", 1_010)
    served = json.loads(body)["documents"][0]
    assert served["fetchedAt"] == "1970-01-01T00:16:40Z"
    assert "fetchedAt" not in document
    assert store.tracked() == {("167", "2026-10-17"): frozenset({"PVD", "KIN"})}


def test_health_fails_when_the_poll_loop_stalls():
    store = TrackStore(started_at=1_000)
    assert route(store, "/tracks.json", 300, 7200, "/healthz", 1_500, 600)[0] == 200
    assert route(store, "/tracks.json", 300, 7200, "/healthz", 1_601, 600)[0] == 503
    store.mark_cycle(1_600)
    assert route(store, "/tracks.json", 300, 7200, "/healthz", 1_700, 600)[0] == 200


def test_health_fails_after_repeated_poll_failures_and_recovers():
    store = TrackStore(started_at=1_000)
    store.record_poll(False)
    store.record_poll(False)
    status, _, body = route(store, "/tracks.json", 300, 7200, "/healthz", 1_010)
    assert (status, body) == (200, b"ok")
    store.record_poll(False)
    status, _, body = route(store, "/tracks.json", 300, 7200, "/healthz", 1_010)
    assert (status, body) == (503, b"polls failing")
    store.record_poll(True)
    status, _, body = route(store, "/tracks.json", 300, 7200, "/healthz", 1_010)
    assert (status, body) == (200, b"ok")
