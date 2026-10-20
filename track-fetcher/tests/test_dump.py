"""The one-shot track dump's parsing, formatting, and request limits. No browser, no network."""

import pytest

from fetcher.dump import format_table, parse_args, posted_tracks, request_pairs

TODAY = "2026-10-19"

# Amtrak's 20261019 shape for train 66: Washington posts a gate beside the track, BWI posted its
# track twelve hours ahead, and a stop with no track yet shows ``TBD``.
DOCUMENT = {
    "data": [
        {
            "stops": [
                {
                    "station": {"code": "WAS", "name": "Washington, DC"},
                    "departure": {
                        "schedule": {"dateTime": "2026-10-19T22:00:00-04:00"},
                        "track": {"number": "24", "asOf": "2026-10-19T21:38:18-04:00"},
                        "gate": {"number": "H", "asOf": "2026-10-19T21:38:18-04:00"},
                    },
                },
                {
                    "station": {"code": "BWI", "name": "BWI Airport, MD"},
                    "arrival": {
                        "schedule": {"dateTime": "2026-10-19T22:33:00-04:00"},
                        "track": {"number": "1", "asOf": "2026-10-19T10:33:01-04:00"},
                    },
                    "departure": {"schedule": {"dateTime": "2026-10-19T22:35:00-04:00"}},
                },
                {
                    "station": {"code": "NYP", "name": "New York, NY"},
                    "arrival": {"track": {"number": "TBD"}},
                },
                {"station": {"code": "BUF"}, "arrival": {"track": {"number": " MAIN "}}},
                "not a stop",
            ]
        }
    ]
}


def test_posted_tracks_lists_every_assigned_track_in_stop_order():
    rows = posted_tracks("66", TODAY, DOCUMENT)
    assert [(row["station"], row["side"], row["track"], row["gate"]) for row in rows] == [
        ("WAS", "departure", "24", "H"),
        ("BWI", "arrival", "1", ""),
        ("BUF", "arrival", "MAIN", ""),
    ]
    assert rows[0] == {
        "train": "66",
        "service_date": TODAY,
        "station": "WAS",
        "station_name": "Washington, DC",
        "side": "departure",
        "track": "24",
        "gate": "H",
        "scheduled": "2026-10-19T22:00:00-04:00",
        "as_of": "2026-10-19T21:38:18-04:00",
    }
    assert rows[2]["scheduled"] == "" and rows[2]["as_of"] == ""


def test_posted_tracks_tolerates_documents_without_stops():
    assert posted_tracks("66", TODAY, {"data": []}) == []
    assert posted_tracks("66", TODAY, {"data": None}) == []
    assert posted_tracks("66", TODAY, {"data": [{"stops": None}, "x"]}) == []


def test_format_table_aligns_columns():
    table = format_table(posted_tracks("66", TODAY, DOCUMENT)).splitlines()
    assert table[0].startswith("Train  Service date  Station  Side")
    assert len(table) == 4
    column = table[0].index("Track")
    assert [line[column:].split()[0] for line in table[1:]] == ["24", "1", "MAIN"]
    assert format_table([]) == "No posted tracks."


def test_arguments_default_to_today_and_deduplicate():
    args = parse_args(["66", "0066", "2150"], TODAY)
    assert args.trains == ["66", "66", "2150"]
    assert args.dates == [TODAY]
    assert request_pairs(args.trains, args.dates) == [("66", TODAY), ("2150", TODAY)]
    args = parse_args(["49", "--date", "2026-10-18", "--date", TODAY], TODAY)
    assert request_pairs(args.trains, args.dates) == [("49", "2026-10-18"), ("49", TODAY)]


@pytest.mark.parametrize(
    "argv",
    [
        ["0"],
        ["12345"],
        ["66/../x"],
        ["66", "--date", "10/19/2026"],
        ["66", "--delay", "-1"],
        ["66", "--timeout", "0"],
    ],
)
def test_bad_arguments_are_rejected(argv):
    with pytest.raises(SystemExit):
        parse_args(argv, TODAY)


def test_request_count_is_capped():
    trains = [str(n) for n in range(1, 22)]
    with pytest.raises(SystemExit):
        parse_args([*trains, "--date", "2026-10-18", "--date", TODAY], TODAY)
    args = parse_args([*trains, "--date", TODAY, "--max-requests", "21"], TODAY)
    assert len(request_pairs(args.trains, args.dates)) == 21
