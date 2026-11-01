"""Status-document parsing, lead time, and the station profile. No browser and no network."""

import json
from pathlib import Path

from fetcher.lead_time import LeadProfile
from fetcher.status_parse import (
    PROFILE_SCHEMA,
    board_field_counts,
    compose_stop,
    coverage_report,
    merge_stop_rows,
    posted_track_number,
    stops_from_document,
    suggested_hot_window,
    suggested_window_before,
)

FIXTURE = json.loads((Path(__file__).parent / "fixtures" / "status-samples.json").read_text())
FETCHED = "2026-11-01T10:00:00Z"


def _train66():
    rows, buses = stops_from_document(
        FIXTURE["train_66"], train="66", service_date="2026-10-31", fetched_at=FETCHED
    )
    return rows, buses


def test_posted_track_number_rejects_blank_and_tbd():
    assert posted_track_number(" 25 ") == "25"
    assert posted_track_number(3) == "3"
    assert posted_track_number(" TBD ") == ""
    assert posted_track_number("tbd") == ""
    assert posted_track_number("") == ""
    assert posted_track_number(None) == ""
    assert posted_track_number(True) == ""


def test_fixture_reads_arrival_and_departure_tracks_gates_and_missing():
    rows, buses = _train66()
    assert buses == 1
    by_station = {row["station"]: row for row in rows}
    assert list(by_station) == ["KIN", "PVD", "WAS", "BOS", "NYP", "BWI"]
    kin = by_station["KIN"]
    assert kin["track"] == "1"
    assert kin["has_track"] is True
    assert kin["has_gate"] is False
    assert kin["lead_time_secs"] == 4 * 3600 + 10 * 60
    assert kin["lead_time_basis"] == "estimated"
    assert kin["chosen_side"] == "arrival"
    pvd = by_station["PVD"]
    assert pvd["has_track"] is False
    assert pvd["track"] == ""
    assert pvd["departure_track"] == ""
    was = by_station["WAS"]
    assert was["arrival_track"] == "25" and was["arrival_gate"] == "G"
    assert was["departure_track"] == "24" and was["departure_gate"] == "H"
    assert was["chosen_side"] == "departure"
    assert was["chosen_track"] == "24"
    assert was["chosen_lead_time_secs"] == 15 * 60
    assert was["side"] == "arrival"
    assert was["track"] == "25"
    assert was["lead_time_secs"] == 5 * 3600 + 55 * 60
    assert was["lead_time_basis"] == "scheduled"
    assert by_station["BWI"]["station"] == "BWI"
    assert by_station["BWI"]["track"] == "3"
    assert by_station["BWI"]["lead_time_secs"] == 6 * 3600
    assert by_station["NYP"]["has_track"] is False
    assert by_station["BOS"]["canceled"] is True
    assert "OKC" not in by_station


def test_a_canceled_train_is_flagged_and_its_track_does_not_count():
    rows, buses = stops_from_document(
        FIXTURE["canceled_821"], train="821", service_date="2026-11-01", fetched_at=FETCHED
    )
    assert buses == 0
    assert len(rows) == 1
    assert rows[0]["canceled"] is True
    assert rows[0]["has_track"] is True
    report = coverage_report(rows, generated_at=FETCHED, snapshot_count=1)
    station = report["stations"][0]
    assert station["station"] == "OKC"
    assert station["stops_seen"] == 0
    assert station["canceled_stops"] == 1
    assert station["stops_with_track"] == 0
    assert station["pct_with_track"] is None
    assert station["confidence"] == "none"


def test_station_summary_ignores_canceled_stops_and_sorts_by_code():
    rows, _ = _train66()
    report = coverage_report(rows, generated_at=FETCHED, snapshot_count=1)
    assert report["schema"] == PROFILE_SCHEMA
    assert report["loaded_by_sidecar"] is False
    assert "TRACK_WINDOW_BEFORE_SECS" in report["consumer"]
    codes = [station["station"] for station in report["stations"]]
    assert codes == ["BOS", "BWI", "KIN", "NYP", "PVD", "WAS"]
    kin = next(station for station in report["stations"] if station["station"] == "KIN")
    assert kin["stops_seen"] == 1
    assert kin["stops_with_track"] == 1
    assert kin["pct_with_track"] == 100.0
    assert kin["lead_median_secs"] == 15000
    assert kin["confidence"] == "sparse"
    assert kin["compared_to_default"] == "wider"
    assert kin["suggested_window_before_secs"] == suggested_window_before(15000)
    assert kin["suggested_hot_window_secs"] == 720
    pvd = next(station for station in report["stations"] if station["station"] == "PVD")
    assert pvd["stops_with_track"] == 0
    assert pvd["pct_with_track"] == 0.0
    assert pvd["confidence"] == "no_track"
    assert pvd["compared_to_default"] == "unknown"
    assert pvd["suggested_window_before_secs"] is None
    was = next(station for station in report["stations"] if station["station"] == "WAS")
    assert was["gates_seen"] == ["G", "H"]
    assert was["tracks_seen"] == ["24", "25"]
    assert was["lead_max_secs"] == 21300
    assert was["chosen_lead_median_secs"] == 900
    assert was["suggested_hot_window_secs"] == suggested_hot_window(900)
    assert was["suggested_window_before_secs"] == 23460


def test_the_emitted_profile_is_what_the_lead_time_loader_adopts(tmp_path: Path):
    """``AMTRAK_TRACKS_LEAD_TIME_FILE`` adopts observed+wider and leaves everyone else global."""
    kin = [
        _kin("66", "2026-11-01T06:00:00-04:00", "2026-11-01T01:00:00-04:00"),
        _kin("95", "2026-11-01T04:00:00-04:00", "2026-11-01T01:00:00-04:00"),
        _kin("67", "2026-11-01T05:00:00-04:00", "2026-11-01T01:00:00-04:00"),
    ]
    sparse = _side_row(
        track="1",
        as_of="2026-11-01T01:00:00-04:00",
        fetched_at=FETCHED,
        scheduled="2026-11-01T08:00:00-04:00",
        train="65",
        station="PVD",
    )
    covered = [
        _side_row(
            track="1",
            as_of="2026-11-01T05:00:00-04:00",
            fetched_at=FETCHED,
            scheduled="2026-11-01T05:10:00-04:00",
            train=train,
            station="BAL",
        )
        for train in ("19", "66", "67")
    ]
    report = coverage_report([*kin, sparse, *covered], generated_at=FETCHED, snapshot_count=1)
    path = tmp_path / "survey.profile.json"
    path.write_text(json.dumps(report), encoding="utf-8")
    loaded = LeadProfile.load(str(path), default_before=1800, default_hot=720)
    kin_row = next(row for row in report["stations"] if row["station"] == "KIN")
    assert kin_row["confidence"] == "observed"
    assert kin_row["compared_to_default"] == "wider"
    assert loaded.before_secs("kin", 1800) == kin_row["suggested_window_before_secs"]
    assert loaded.hot_secs("KIN", 720) == kin_row["suggested_hot_window_secs"]
    assert loaded.before_secs("PVD", 1800) == 1800
    assert loaded.before_secs("BAL", 1800) == 1800
    assert loaded.hot_secs("BAL", 720) == 720


def test_a_stamp_after_the_event_does_not_suggest_a_lookahead():
    assert suggested_window_before(0) is None
    assert suggested_window_before(-350) is None
    rows = [
        _side_row(
            track="24",
            as_of="2026-10-31T16:34:50-04:00",
            fetched_at=FETCHED,
            scheduled="2026-10-31T16:29:00-04:00",
            train=train,
            station="WAS",
        )
        for train in ("19", "40", "66")
    ]
    station = coverage_report(rows, generated_at=FETCHED, snapshot_count=1)["stations"][0]
    assert station["lead_max_secs"] < 0
    assert station["confidence"] == "observed"
    assert station["suggested_window_before_secs"] is None
    assert station["compared_to_default"] == "unknown"


def test_three_leads_are_observed_and_the_window_uses_the_longest():
    rows = [
        _kin("66", "2026-11-01T05:00:00-04:00", "2026-11-01T01:00:00-04:00"),
        _kin("95", "2026-11-01T04:00:00-04:00", "2026-11-01T01:00:00-04:00"),
        _kin("67", "2026-11-01T06:00:00-04:00", "2026-11-01T01:00:00-04:00"),
    ]
    station = coverage_report(rows, generated_at=FETCHED, snapshot_count=2)["stations"][0]
    assert station["train_numbers"] == ["66", "67", "95"]
    assert station["lead_min_secs"] == 3 * 3600
    assert station["lead_median_secs"] == 4 * 3600
    assert station["lead_max_secs"] == 5 * 3600
    assert station["confidence"] == "observed"
    assert station["suggested_window_before_secs"] == suggested_window_before(5 * 3600)
    assert station["suggested_hot_window_secs"] == 720
    assert station["compared_to_default"] == "wider"


def test_merge_keeps_the_newest_number_and_the_earliest_stamp_of_that_number():
    early = _side_row(
        track="1",
        as_of="2026-11-01T01:00:00-04:00",
        fetched_at="2026-11-01T12:00:00Z",
        scheduled="2026-11-01T06:00:00-04:00",
    )
    late = _side_row(
        track="2",
        as_of="2026-11-01T05:00:00Z",
        fetched_at="2026-11-01T16:00:00Z",
        scheduled="2026-11-01T06:30:00-04:00",
    )
    merged = merge_stop_rows([[late], [early]])
    assert len(merged) == 1
    assert merged[0]["track"] == "2"
    assert merged[0]["track_as_of"] == "2026-11-01T05:00:00Z"
    assert merged[0]["scheduled"] == "2026-11-01T06:30:00-04:00"

    first = _side_row(
        track="1",
        as_of="2026-11-01T01:30:00-04:00",
        fetched_at="2026-11-01T12:00:00Z",
        scheduled="2026-11-01T06:00:00-04:00",
    )
    second = _side_row(
        track="1",
        as_of="2026-11-01T05:00:00Z",
        fetched_at="2026-11-01T18:00:00Z",
        scheduled="2026-11-01T06:30:00-04:00",
    )
    # 05:00Z is 01:00-04:00, which is before 01:30-04:00. String order would pick the other one.
    same = merge_stop_rows([[first], [second]])
    assert same[0]["track_as_of"] == "2026-11-01T05:00:00Z"
    assert same[0]["lead_time_secs"] == 5 * 3600 + 30 * 60

    cleared = _side_row(
        track="",
        as_of="",
        fetched_at="2026-11-01T20:00:00Z",
        scheduled="2026-11-01T06:40:00-04:00",
    )
    kept = merge_stop_rows([[first], [cleared]])
    assert kept[0]["has_track"] is True
    assert kept[0]["track"] == "1"
    assert kept[0]["scheduled"] == "2026-11-01T06:40:00-04:00"


def test_stop_order_is_station_then_date_then_train_number():
    late = _side_row(
        track="1",
        as_of="2026-11-01T01:00:00-04:00",
        fetched_at=FETCHED,
        scheduled="2026-11-01T06:00:00-04:00",
        train="100",
        service_date="2026-11-01",
        station="KIN",
    )
    early = _side_row(
        track="2",
        as_of="2026-11-01T01:00:00-04:00",
        fetched_at=FETCHED,
        scheduled="2026-11-01T06:00:00-04:00",
        train="66",
        service_date="2026-10-31",
        station="WAS",
    )
    middle = _side_row(
        track="3",
        as_of="2026-11-01T01:00:00-04:00",
        fetched_at=FETCHED,
        scheduled="2026-11-01T06:00:00-04:00",
        train="67",
        service_date="2026-11-01",
        station="KIN",
    )
    ordered = merge_stop_rows([[late, early, middle]])
    assert [(row["station"], row["service_date"], row["train"]) for row in ordered] == [
        ("KIN", "2026-11-01", "67"),
        ("KIN", "2026-11-01", "100"),
        ("WAS", "2026-10-31", "66"),
    ]


def test_board_field_counts_notice_a_missing_track():
    present = {"travelLegs": [{"origin": {"track": {"number": "12"}, "gate": {"number": "W"}}}]}
    absent = {"travelLegs": [{"origin": {"statusInfo": {"status": "ON TIME"}}}]}
    tbd = {"travelLegs": [{"origin": {"track": {"number": "TBD"}, "gate": {"number": "TBD"}}}]}
    assert board_field_counts(present) == {
        "has_track": True,
        "has_gate": True,
        "track_count": 1,
        "gate_count": 1,
    }
    assert board_field_counts(absent)["has_track"] is False
    assert board_field_counts(absent)["has_gate"] is False
    assert board_field_counts(tbd)["has_track"] is False
    assert board_field_counts(tbd)["has_gate"] is False


def _kin(train: str, scheduled: str, as_of: str) -> dict:
    return _side_row(
        track="1",
        as_of=as_of,
        fetched_at=FETCHED,
        scheduled=scheduled,
        train=train,
        service_date="2026-10-31",
        station="KIN",
    )


def _side_row(
    *,
    track: str,
    as_of: str,
    fetched_at: str,
    scheduled: str,
    train: str = "66",
    service_date: str = "2026-10-31",
    station: str = "KIN",
) -> dict:
    arrival = {
        "side": "arrival",
        "scheduled": scheduled,
        "estimated": "",
        "time_basis": "scheduled",
        "track": track,
        "track_as_of": as_of,
        "gate": "",
        "gate_as_of": "",
        "canceled": False,
        "lead_time_secs": None,
        "lead_time_basis": "",
    }
    return compose_stop(
        train=train,
        service_date=service_date,
        stop_index=0,
        station=station,
        station_name="Kingston, RI" if station == "KIN" else station,
        fetched_at=fetched_at,
        arrival=arrival,
        departure=None,
    )
