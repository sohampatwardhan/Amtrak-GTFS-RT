"""Early track refreshes debounce, coalesce, and stay inside the fetch cap."""

from worker.tracks.schedule import take_early


def test_debounce_coalesces_and_the_cap_and_gap_hold():
    touches = {"167": 0.0, "66": 1.0}
    refreshed: dict[str, float] = {}
    assert take_early(touches, refreshed, 4.0, debounce_secs=5, min_gap_secs=90, max_per_flush=24) == []
    touches["167"] = 3.0
    assert take_early(touches, refreshed, 8.0, debounce_secs=5, min_gap_secs=90, max_per_flush=24) == [
        "167",
        "66",
    ]
    touches["167"] = 9.0
    assert take_early(touches, refreshed, 20.0, debounce_secs=5, min_gap_secs=90, max_per_flush=24) == []
    assert take_early(touches, refreshed, 100.0, debounce_secs=5, min_gap_secs=90, max_per_flush=24) == [
        "167"
    ]

    burst = {str(number): 0.0 for number in range(30)}
    released: dict[str, float] = {}
    first = take_early(burst, released, 5.0, debounce_secs=0, min_gap_secs=90, max_per_flush=24)
    assert len(first) == 24
    second = take_early(burst, released, 6.0, debounce_secs=0, min_gap_secs=0, max_per_flush=24)
    assert len(second) == 6
