"""Lead-time profiles adopt only observed stations that need a wider window."""

import json
from pathlib import Path

from fetcher.lead_time import SCHEMA, LeadProfile


def test_pvd_hot_window_is_tighter_and_sparse_stations_stay_global(tmp_path: Path):
    path = tmp_path / "stations.profile.json"
    path.write_text(
        json.dumps(
            {
                "schema": SCHEMA,
                "stations": [
                    {
                        "station": "PVD",
                        "confidence": "observed",
                        "compared_to_default": "wider",
                        "suggested_window_before_secs": 600,
                        "suggested_hot_window_secs": 180,
                    },
                    {
                        "station": "NYP",
                        "confidence": "sparse",
                        "compared_to_default": "wider",
                        "suggested_window_before_secs": 9000,
                        "suggested_hot_window_secs": 60,
                    },
                    {
                        "station": "BOS",
                        "confidence": "no_track",
                        "compared_to_default": "unknown",
                        "suggested_window_before_secs": 9000,
                        "suggested_hot_window_secs": 60,
                    },
                ],
            }
        ),
        encoding="utf-8",
    )
    profile = LeadProfile.load(str(path), default_before=1800, default_hot=720)
    assert profile.before_secs("PVD", 1800) == 600
    assert profile.hot_secs("pvd", 720) == 180
    assert profile.before_secs("NYP", 1800) == 1800
    assert profile.before_secs("ZZZ", 1800) == 1800
    assert LeadProfile.load(None, default_before=1800, default_hot=720).before_secs("PVD", 1800) == 1800
