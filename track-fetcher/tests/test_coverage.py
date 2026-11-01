"""Request order, the coverage report files, and argument limits. No browser and no network."""

import json

import pytest

from fetcher.coverage import (
    build_payload,
    format_station_table,
    jobs_from_dates,
    jobs_from_fleet,
    merge_command,
    next_pause,
    parse_args,
    read_fetch_result,
    write_report,
)
from fetcher.select import list_fleet, origin_service_date
from fetcher.status_parse import STOP_FIELDS, STATION_FIELDS


def test_origin_date_wins_over_a_stop_after_midnight():
    properties = {
        "TrainNum": "0066",
        "OrigSchDep": "10/31/2026 8:00:00 PM",
        "Station1": {"code": "RNK", "bus": False, "schdep": "10/31/2026 20:00:00"},
        "Station2": {"code": "KIN", "bus": False, "scharr": "11/01/2026 05:18:00"},
    }
    assert origin_service_date(properties) == "2026-10-31"
    fallback = {
        "TrainNum": "66",
        "Station1": {"code": "KIN", "bus": False, "scharr": "11/01/2026 05:18:00"},
    }
    assert origin_service_date(fallback) == "2026-11-01"


def test_fleet_order_is_origin_date_then_train_number():
    geojson = {
        "features": [
            {
                "properties": {
                    "TrainNum": "67",
                    "OrigSchDep": "11/01/2026 6:00:00 AM",
                    "Station1": {"code": "WAS", "bus": False},
                }
            },
            {
                "properties": {
                    "TrainNum": "A0066",
                    "OrigSchDep": "10/31/2026 8:00:00 PM",
                    "Station1": {"code": "kin", "bus": False},
                    "Station2": {"code": "WAS", "bus": False},
                }
            },
            {
                "properties": {
                    "TrainNum": "100",
                    "OrigSchDep": "10/31/2026 9:00:00 AM",
                    "Station1": {"code": "NYP", "bus": False},
                }
            },
            {
                "properties": {
                    "TrainNum": "3167",
                    "OrigSchDep": "11/01/2026 1:00:00 PM",
                    "Station1": {"code": "BOS", "bus": True},
                }
            },
            {
                "properties": {
                    "TrainNum": "66",
                    "OrigSchDep": "10/31/2026 8:00:00 PM",
                    "Station1": {"code": "NYP", "bus": False},
                }
            },
        ]
    }
    trains, bus_only = list_fleet(geojson)
    assert bus_only == 1
    assert [(train.train_number, train.service_date) for train in trains] == [
        ("66", "2026-10-31"),
        ("100", "2026-10-31"),
        ("67", "2026-11-01"),
    ]
    assert trains[0].station_codes == ("KIN", "WAS")
    chosen, skipped, matched = jobs_from_fleet(
        trains, trains=None, stations={"KIN"}, offset=0, limit=10
    )
    assert chosen == [("66", "2026-10-31")]
    assert skipped == []
    assert matched == 1
    chosen, skipped, matched = jobs_from_fleet(
        trains, trains=None, stations=None, offset=1, limit=1
    )
    assert chosen == [("100", "2026-10-31")]
    assert skipped == [("67", "2026-11-01")]
    assert matched == 3


def test_named_trains_are_requested_by_date_then_number():
    chosen, skipped, matched = jobs_from_dates(
        ["2150", "66", "66"],
        ["2026-11-01", "2026-10-31"],
        offset=1,
        limit=2,
    )
    assert matched == 4
    assert chosen == [("2150", "2026-10-31"), ("66", "2026-11-01")]
    assert skipped == [("2150", "2026-11-01")]


def test_watch_sleep_stops_at_the_deadline():
    assert next_pause(elapsed=400, interval=900, time_left=3200) == 500
    assert next_pause(elapsed=400, interval=900, time_left=500) is None
    assert next_pause(elapsed=1000, interval=900, time_left=100) == 0
    assert next_pause(elapsed=10, interval=900, time_left=0) is None
    assert next_pause(elapsed=10, interval=900, time_left=-5) is None


def test_fetch_results_distinguish_akamai_from_a_train_that_does_not_run():
    blocked = read_fetch_result({"status": 403, "body": "denied"})
    assert blocked["fatal"] is True
    assert blocked["error"] == "http 403"
    missing = read_fetch_result(
        {
            "status": 200,
            "body": json.dumps(
                {"error": {"code": "ODH-30003", "message": "Service does not operate on requested date."}}
            ),
        }
    )
    assert missing["fatal"] is False
    assert missing["document"] is None
    assert missing["error"] == "Service does not operate on requested date."
    server = read_fetch_result({"status": 500, "body": ""})
    assert server["fatal"] is False
    assert server["error"] == "http 500"
    assert read_fetch_result(None)["fatal"] is True


def test_report_files_sort_stops_and_write_the_profile(tmp_path):
    later = {
        "fetched_at": "2026-11-01T10:00:00Z",
        "train": "100",
        "service_date": "2026-11-01",
        "stop_index": 0,
        "station": "WAS",
        "station_name": "Washington, DC",
        "canceled": False,
        "has_track": False,
        "has_gate": False,
        "side": "arrival",
        "track": "",
        "track_as_of": "",
        "gate": "",
        "gate_as_of": "",
        "scheduled": "",
        "estimated": "",
        "lead_time_secs": None,
        "lead_time_basis": "",
        "chosen_side": "arrival",
        "chosen_track": "",
        "chosen_track_as_of": "",
        "chosen_gate": "",
        "chosen_lead_time_secs": None,
        "arrival_scheduled": "",
        "arrival_estimated": "",
        "arrival_time_basis": "",
        "arrival_track": "",
        "arrival_track_as_of": "",
        "arrival_gate": "",
        "arrival_gate_as_of": "",
        "arrival_canceled": False,
        "arrival_lead_time_secs": None,
        "departure_scheduled": "",
        "departure_estimated": "",
        "departure_time_basis": "",
        "departure_track": "",
        "departure_track_as_of": "",
        "departure_gate": "",
        "departure_gate_as_of": "",
        "departure_canceled": False,
        "departure_lead_time_secs": None,
    }
    earlier = {**later, "train": "66", "service_date": "2026-10-31", "station": "KIN"}
    payload = build_payload(
        [later, earlier],
        kind="survey",
        fetched_at="2026-11-01T10:00:00Z",
        delay_secs=2.0,
        max_requests=300,
        offset=0,
        truncated=False,
        matched_trains=2,
        station_filter=["KIN"],
        missing_trains=[],
        not_fetched=[],
        bus_only_skipped=0,
        bus_services_skipped=0,
        status_documents=2,
        aborted=None,
        sources=["2026-11-01T10:00:00Z"],
        input_files=[],
        inputs_aborted=0,
        requests=[],
        station_board={"probed": False},
        snapshot_count=1,
    )
    assert [(row["station"], row["train"]) for row in payload["stops"]] == [("KIN", "66"), ("WAS", "100")]
    assert payload["profile"]["loaded_by_sidecar"] is False
    assert payload["profile"]["schema"] == "amtrak-station-lead-time/v1"
    path = tmp_path / "survey.json"
    write_report(path, payload)
    write_report(path, payload)
    text = path.read_text(encoding="utf-8")
    assert text == path.read_text(encoding="utf-8")
    assert json.loads(text)["stops"][0]["station"] == "KIN"
    assert (tmp_path / "survey.csv").read_text(encoding="utf-8").splitlines()[0] == ",".join(STOP_FIELDS)
    station_header = (tmp_path / "survey.stations.csv").read_text(encoding="utf-8").splitlines()[0]
    assert station_header == ",".join(STATION_FIELDS)
    profile = json.loads((tmp_path / "survey.profile.json").read_text(encoding="utf-8"))
    assert profile["loaded_by_sidecar"] is False
    table = format_station_table(payload["stations"])
    assert table.splitlines()[1].startswith("KIN")
    assert table.splitlines()[2].startswith("WAS")


def test_merge_of_a_directory_skips_an_older_merged_file(tmp_path):
    kin = _snapshot("KIN", "66")
    was = _snapshot("WAS", "67")
    extra = _snapshot("ZZZ", "1")
    extra["kind"] = "merged"
    (tmp_path / "a.json").write_text(json.dumps(kin), encoding="utf-8")
    (tmp_path / "b.json").write_text(json.dumps(was), encoding="utf-8")
    (tmp_path / "merged.json").write_text(json.dumps(extra), encoding="utf-8")
    (tmp_path / "ignored.profile.json").write_text(json.dumps(extra), encoding="utf-8")
    out = tmp_path / "out" / "coverage.json"
    assert merge_command(type("A", (), {"inputs": [str(tmp_path)], "out": str(out)})()) == 0
    payload = json.loads(out.read_text(encoding="utf-8"))
    assert [row["station"] for row in payload["stops"]] == ["KIN", "WAS"]
    assert payload["kind"] == "merged"
    assert payload["snapshot_count"] == 2
    named = tmp_path / "only-merged.json"
    assert merge_command(type("A", (), {"inputs": [str(tmp_path / "merged.json")], "out": str(named)})()) == 0
    named_payload = json.loads(named.read_text(encoding="utf-8"))
    assert [row["station"] for row in named_payload["stops"]] == ["ZZZ"]


@pytest.mark.parametrize(
    "argv",
    [
        ["tracks", "66", "--delay", "0.5"],
        ["tracks", "66", "--delay", "61"],
        ["tracks", "66", "--max-requests", "0"],
        ["tracks", "66", "--max-requests", "501"],
        ["tracks", "66", "--offset", "-1"],
        ["tracks", "0"],
        ["survey", "--delay", "2"],
        ["survey", "--interval", "30", "--duration", "120", "--out-dir", "snaps"],
        ["survey", "--interval", "900", "--out-dir", "snaps"],
        ["survey", "--interval", "900", "--duration", "3600", "--out", "x.json", "--out-dir", "snaps"],
        ["survey", "--max-cycles", "0", "--interval", "900", "--duration", "3600", "--out-dir", "snaps"],
    ],
)
def test_limits_and_required_outputs_are_rejected(argv):
    with pytest.raises(SystemExit):
        parse_args(argv)


def test_survey_and_tracks_accept_a_polite_cap():
    survey = parse_args(["survey", "--out", "survey.json", "--station", "kin", "--delay", "1"])
    assert survey.delay == 1
    assert survey.max_requests == 300
    assert survey.station == ["KIN"]
    assert survey.no_board is False
    tracks = parse_args(["tracks", "0066", "2150", "--date", "2026-10-31", "--max-requests", "40"])
    assert tracks.trains == ["66", "2150"]
    assert tracks.dates == ["2026-10-31"]
    watching = parse_args(
        ["survey", "--interval", "900", "--duration", "3600", "--out-dir", "snapshots", "--no-board"]
    )
    assert watching.interval == 900
    assert watching.no_board is True


def _snapshot(station: str, train: str) -> dict:
    stop = {
        "fetched_at": "2026-11-01T10:00:00Z",
        "train": train,
        "service_date": "2026-10-31",
        "stop_index": 0,
        "station": station,
        "station_name": station,
        "canceled": False,
        "has_track": True,
        "has_gate": False,
        "side": "arrival",
        "track": "1",
        "track_as_of": "2026-11-01T01:00:00-04:00",
        "gate": "",
        "gate_as_of": "",
        "scheduled": "2026-11-01T05:00:00-04:00",
        "estimated": "",
        "lead_time_secs": 4 * 3600,
        "lead_time_basis": "scheduled",
        "chosen_side": "arrival",
        "chosen_track": "1",
        "chosen_track_as_of": "2026-11-01T01:00:00-04:00",
        "chosen_gate": "",
        "chosen_lead_time_secs": 4 * 3600,
        "arrival_scheduled": "2026-11-01T05:00:00-04:00",
        "arrival_estimated": "",
        "arrival_time_basis": "scheduled",
        "arrival_track": "1",
        "arrival_track_as_of": "2026-11-01T01:00:00-04:00",
        "arrival_gate": "",
        "arrival_gate_as_of": "",
        "arrival_canceled": False,
        "arrival_lead_time_secs": 4 * 3600,
        "departure_scheduled": "",
        "departure_estimated": "",
        "departure_time_basis": "",
        "departure_track": "",
        "departure_track_as_of": "",
        "departure_gate": "",
        "departure_gate_as_of": "",
        "departure_canceled": False,
        "departure_lead_time_secs": None,
    }
    return {
        "kind": "survey",
        "fetched_at": "2026-11-01T10:00:00Z",
        "truncated": False,
        "matched_trains": 1,
        "station_filter": [],
        "bus_only_skipped": 0,
        "bus_services_skipped": 0,
        "status_documents": 1,
        "aborted": None,
        "station_board": {"probed": False},
        "stops": [stop],
    }
