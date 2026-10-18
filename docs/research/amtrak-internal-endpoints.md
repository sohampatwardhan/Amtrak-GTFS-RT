# Amtrak internal endpoints

Observed 2026-10-17, about 19:37–19:48 America/New_York, with one headless Chromium session. Pages loaded the way a rider would: the home page, Train Status for a public train number, Track Your Train, Service Alerts & Notices (one route advisory and one station advisory), and the New York station page (`/stations/nyp`). A few same-origin reads then repeated URLs those pages, or the booking bundle, already reference. No login, no load test.

Plain `urllib` to `www.amtrak.com` timed out (20 seconds) on the station directory, the route list, the homepage-alerts document, and a train-status URL. The same URLs returned 200 inside Chromium. `maps.amtrak.com` did not: `GET /rttl/js/RoutesList.json` returned 200 in under a second with a normal user agent and no cookies. That split matches how this service already works. The map feed is fetched with an ordinary HTTP client. Anything under `www.amtrak.com` needs the same kind of headless browser the advisory fetcher and the PR #22 track sidecar use.

The statuses request Chromium captured sent `accept`, `content-type: application/json`, `referer`, a browser user agent, and `x-amtrak-trace-id` (a client-generated id). No `Authorization` header was present. The capture does not show a `Cookie` header; that may be the tool omitting it. Treat Akamai as the gate, not an API key.

## What is worth integrating

| Endpoint | New GTFS-RT value | Poll |
| --- | --- | --- |
| `GET /dotcom/travel-service/statuses/{number}?service-date=` | Actual vs estimate (`dateTimeType`, `autoCalculated`), per-stop cancellation (sometimes weeks ahead), track, operational comment. A substitute bus is a separate `BUS` service, not a flag on the train. Per-stop delay is not new: trip updates already carry estimated times from `getTrainsData`. | Only trains already in the track sidecar's window, plus numbers named by an advisory. About once a minute near a stop; every 10–15 minutes for a future cancellation. |
| `GET /dotcom/travel-service/statuses/stations?origin-code=&destination-code=&departure-date=` | One corridor board: tracks, **gate letters**, delays, and cancellations for every train between two stations. Origin and destination only. | A few major pairs (NYP–WAS, NYP–BOS, and similar), every 2–5 minutes. |
| Existing `getTrainsData` | Live position, speed, bearing. The status document's `locationInfo` was still the origin station after the train had left. | Keep the current fleet poll. Do not replace it. |
| HTML `/alert` and `/alert/{slug}.html` | The only place that ties a canceled train number to the substitute bus number. Already consumed by the advisory fetcher. | List every 5–10 minutes. Open a detail page when the list changes. |

Skip the rest for GTFS-RT. Reference JSON, map geometry, and the station marketing page are static. The AEM "alert check" URLs returned empty. Booking, profile, and disruption URLs need a signed-in session and were not called.

## Baseline this service already has

`maps.amtrak.com` `getTrainsData`, decrypted by `catenarytransit/amtrak-gtfs-rt`, is one fleet document. Each train feature carries a coordinate, speed, bearing, route name, and `Station0..N` records with `code`, `tz`, `bus`, scheduled / estimated / posted times, and comments. `schcmnt: "Canceled"` is the cancellation flag. There is no track or gate field. PR #22 reads track numbers from `stops[].departure.track.number` (else `arrival.track`) on the per-train status document below.

The advisory fetcher reads `https://www.amtrak.com/alert` (and the configured alerts page) as HTML.

## Train status, one service

```
GET https://www.amtrak.com/dotcom/travel-service/statuses/{number}?service-date=YYYY-MM-DD
```

Called by the train-status page after the stop query below. `cache-control: no-cache, no-store, must-revalidate`. AWS API Gateway sits behind Akamai (`x-amz-apigw-id`). No API key.

Train 49, Lake Shore Limited, service date 2026-10-17, fetched about 19:42 ET. Body about 16 KB. Shape:

```json
{
  "data": [
    {
      "id": "004920261017",
      "travelService": {
        "number": "49",
        "date": "2026-10-17",
        "type": { "code": "TRN", "description": "Intercity Train" },
        "name": { "code": "LSLM", "description": "Lake Shore Limited" },
        "operator": { "code": "AMTK", "description": "Amtrak" },
        "origin": { "code": "NYP", "timeZone": "America/New_York" },
        "destination": { "code": "CHI", "timeZone": "America/Chicago" },
        "directionCode": "W",
        "daysOfOperation": "SMTWTFS"
      },
      "statusSummary": {
        "displayMessage": "8 MIN LATE",
        "locationInfo": {
          "lastKnownLocationCode": "NYP",
          "latitude": 40.751038,
          "longitude": -73.996327
        }
      },
      "stops": []
    }
  ]
}
```

`id` is the train number zero-padded to four digits plus `YYYYMMDD`. Each stop id appends the station code (`004920261017NYP`). Twenty stops on this train. A stop has `stopNumber`, `boarding`, `deboarding`, `station` (`code`, `name`, `facility`, `timeZone`, `baggageService`), and `arrival` and/or `departure`.

An event:

```json
{
  "schedule": { "dateTime": "2026-10-17T20:45:00-04:00" },
  "statusInfo": {
    "status": "DELAYED",
    "displayStatus": "Now 08:53p",
    "displayMessage": "8 Minutes Late",
    "dateTimeType": "ESTIMATE",
    "dateTime": "2026-10-17T20:53:00-04:00",
    "delay": "PT8M",
    "asOf": "2026-10-17T19:35:00-04:00",
    "autoCalculated": false
  },
  "track": { "number": "1", "asOf": "2026-10-17T01:19:43-04:00" }
}
```

`delay` is an ISO-8601 duration. Early is negative (`PT-37M` at Chicago). `dateTimeType` was `ACTUAL` for stops already passed and `ESTIMATE` ahead. `status` on this train was `ON TIME`, `DELAYED`, or `EARLY`. `autoCalculated: false` on the next stop's arrival; many later estimates were `autoCalculated: true`, including a 37-minute-early Chicago estimate that reads as schedule recovery rather than a live prediction. Prefer `ACTUAL`, and `ESTIMATE` only when `autoCalculated` is false or the stop is the next one.

Freshness on this document: estimates carried `asOf` 19:35 ET, about seven minutes before the fetch. Actuals were stamped within a few minutes of the event (Albany departure actual `asOf` 19:03 ET for a 19:05 departure). Track stamps were older: Rhinecliff track 1 and Waterloo track 1 at 01:19 ET, Buffalo `MAIN` at 12:29 ET. That matches PR #22: some tracks are posted hours ahead and stay on the document.

`statusSummary.locationInfo` was still New York Penn's coordinate while the page's next stop was Utica. It is not a substitute for the map feed's moving position.

`statusInfo.comment` is a free-text operations note. South Bend had `(4211 17OCT26) COVERED BY CARETAKER. NO TICKETING.` Not a personal record. Useful as an alert on that stop, not as a delay.

A train that does not run returns 200 with an error, not an empty `data` array. Train 67 at New York on 2026-10-17 (a Saturday it does not run):

```json
{
  "error": {
    "code": "ODH-30003",
    "message": "Service does not operate on requested date.",
    "businessMessage": "The train number entered does not operate on the date selected. Check the number and date, or try a search using two stations."
  }
}
```

### Cancellation and the substitute bus

The Heartland Flyer advisory for 2026-10-18 says train 821 is canceled and bus 3821 covers Oklahoma City–Fort Worth. Both numbers are in this API, as two services.

Train 821, fetched the evening of the 17th, is still `type.code: "TRN"` / Heartland Flyer. All seven stops are `status: "CANCELED"`, `displayStatus: "Canceled"`, `displayMessage: "Service Canceled"`. `asOf` is `2026-10-02T08:17:12-05:00`, so the cancellation was published about two weeks ahead. `statusSummary` was `{}`. There is no pointer at the bus.

Bus 3821 on the same date:

```json
{
  "travelService": {
    "number": "3821",
    "type": { "code": "BUS", "description": "Intercity Bus" },
    "name": { "code": "TEMP", "description": "Amtrak Thruway Connecting Service" }
  }
}
```

Same seven stations (OKC, NOR, PUR, PVL, ADM, GLE, FTW). Each event is `status: "SCHEDULED"`, `displayMessage: "On Time"`. No track. The map feed's per-stop `bus` flag is not this: the substitute is its own service number, and it will not appear as a moving train until it operates. The HTML advisory is what links 821 to 3821.

### Stop-scoped lookup the form uses first

```
GET /dotcom/travel-service/statuses/stops/{station}?service-numbers={number}&departure-date=YYYY-MM-DD
```

The home-page Train Status form calls this, then the full document. It is the same family of payload, limited to the station you typed. Redundant once the full document is fetched. The date query name here is `departure-date`, not `service-date`. The booking bundle also substitutes `{dateType}-date`, so `arrival-date` is the other form. Not called.

## Corridor board

```
GET /dotcom/travel-service/statuses/stations?origin-code={ORG}&destination-code={DST}&departure-date=YYYY-MM-DD
```

Also `no-store`. NYP to WAS on 2026-10-17 returned 32 services, about 58 KB: 26 `TRN` and 6 `HST` (Acela). Each item is `{ "travelLegs": [ { "travelService", "origin", "destination" } ] }`. Only the two ends of the searched pair, not the stops in between.

Across the origin and destination stops in that one NYP–WAS response: 35 tracks, 35 gates, and status counts `ON TIME` 29, `DELAYED` 27, `EARLY` 18, `CANCELED` 12. A gate was not on the Lake Shore document; it was common on this Northeast Corridor board. Train 109 departing New York:

```json
{
  "gate": { "number": "W", "asOf": "2026-10-17T05:20:14-04:00" },
  "track": { "number": "12", "asOf": "2026-10-17T05:20:18-04:00" }
}
```

Washington arrival for the same train was an actual, `delay: "PT12M"`. The board keeps trains that already finished that service date, so one call is the day's history for that pair, not only what is still coming.

This is the cheapest way to see gates and same-day cancellations for a corridor. It does not replace the per-train document for intermediate stops, and it will not show a bus that does not run between the two codes you asked for.

## Track Your Train map

`https://www.amtrak.com/track-your-train.html` still draws the map from `maps.amtrak.com`. Observed on that page load:

| URL | What it is | Session |
| --- | --- | --- |
| `GET /services/MapDataService/trains/getTrainsData` | The encrypted fleet feed. `text/plain`, about 1 MB of ciphertext. No `cache-control` on this response. | The existing poller already fetches this without a browser. |
| `GET /services/MapDataService/stations/trainStations` | Encrypted blob, about 360 KB compressed. Station layer for the map, not a new realtime feed. | Same host as the fleet feed. |
| `GET /services/MapDataService/stations/nationalRoute?routeName=` | Route geometry. `application/json`, about 2.3 MB compressed. `cache-control: max-age=69541` (about 19 hours). Body not downloaded. | Static. |
| `GET /rttl/js/RoutesList.json` | 38 map routes: `Name`, `CMSID`, `ZoomLevel`. Includes Floridian and Amtrak Mardi Gras Service. | Plain HTTP works. `max-age` about 7 hours on the direct read. |
| `GET /rttl/js/route_properties.json` | 83 features, geometry null, bounding boxes (`NAME`, `SEGMENTID`, `NORTH`, `SOUTH`, `EAST`, `WEST`). Map labels. | Static. |
| `GET /rttl/js/RoutesList.v.json` | Opaque version material the map uses with the ciphertext (`arr`, `s`, `v`). Not a data feed. Values omitted here. | Leave it to the existing decrypt path. |

Nothing on this page adds delay, track, gate, cancellation text, or bus substitutions beyond `getTrainsData`.

## Service alerts

`https://www.amtrak.com/alert` is server-rendered HTML. The list names the route, the title, the effective dates, and a tooltip of affected routes ("Multiple Routes" covers several). Opening an item goes to another HTML page, for example `/alert/heartland-flyer-service-adjustments.html` and `/alert/boston-south-station-restroom-improvement-project.html`. Those pages did not call a JSON status API. Station advisories on the list that evening included ALX, BOS, CHI, DLD, HMW, LAB, MEM, MET, NBK, NHV, OTM, PJC, QAN, and WNL. This is the source the advisory fetcher already parses. It is also the index of train and bus numbers worth a status fetch.

Two JSON checks the booking bundle uses during search returned nothing useful:

```
GET /services/contentService.alerts.stationalerts.check.departDate-20261018.stations-NYP|WAS.json
→ {"stationAlert":""}    cache max-age about 6 hours

GET /services/contentService.alerts.routealerts.check.departDate-20261018.routeName-Heartland%20Flyer.json
→ empty body, 200, max-age 86400

GET ...routeName-Northeast%20Regional.json for 20261017
→ empty body, 200
```

The client treats a non-empty `stationAlert` or `serviceAlert` string as a URL. Tonight the public HTML list was the one that actually had advisories.

Homepage banners, both empty at 23:37 UTC:

```
GET /content/amtrak/en-us/home.alertsService.emergency-alerts.json
GET /content/amtrak/en-us/home.alertsService.homepage-alerts.json
→ {"alerts":[]}
```

`homepage-alerts` was `last-modified` two minutes earlier and `cache-control: max-age=167`. A non-empty banner was not observed, so the field list inside `alerts[]` is unknown. One GET every few minutes is enough if someone wants to catch a banner the HTML list does not show. It is not a replacement for `/alert`.

## Station and reference documents

These are directories and facility copy. They do not move with the trains.

| URL | Observed shape | Freshness | GTFS-RT |
| --- | --- | --- | --- |
| `GET /services/data.stations.json` | 1,031 stations. `code`, `name`, `city`, `state`, `timezone` (`E`/`C`/`M`/`P`, plus `MS`), `autoFillName`. Some rows add `facilityName32`, `facilityName64`, `stationAlias`, `country` (`CA`), `dailyDepartures` (a display string such as `3 daily departures`). | `last-modified` about 9 hours before the fetch, `max-age=55070`. | Station-code directory only. |
| `GET /services/contentService.stationsinfo.json` | `{ "stations": [ { stationCode, quikTrakAvailable, staffed, stationAccessible, wheelChairAvailable } ] }`, same 1,031 codes. | Served with the directory. | Static accessibility flags, coarser than GTFS. |
| `GET /services/routes-list.json` | `{ "RoutesList": [ { routeCode, routeName, cityServed } ] }`, 53 routes. `LSLM` is Lake Shore Limited, the same `name.code` as on the status document. `COAS` is Coast Starlight. | Small, loaded with the home page. | Names, not operations. |
| `GET /services/data.popularstations.json` | Nine origins and their destination codes for the booking UI. | Static. | None. |
| `GET /content/amtrak/en-us/reference-data/wsdot-stations-api.jsondata.json` | Cascades station codes for WSDOT. | Static. | None. |
| `GET /content/amtrak/en-us/stations/nyp.stationTabContainer.NYP.json` | What `/stations/nyp` actually requested. `parking`, `hours`, `baggage`, `feature`, `accessibility`, each a list of display strings. | `max-age=80801`, `last-modified` about 90 minutes earlier. | Marketing copy. |
| `GET /dotcom/stations/{code}/all` | Facility master the booking client calls (`StationInfoURL`). NYP was about 12 KB: `effectiveDate`, `active`, hours by weekday, baggage, parking, accessibility, thruway. The document also has staff names and phone numbers. | `no-store` on this read. | Do not poll and do not log. Not a realtime feed. |

`GET /services/staticmappingjson` is booking-form configuration (fare labels, card rules). No operational data, and it should not be copied into a feed.

The home page also requested `TrainCodesResource.xml` and `/services/globalerrorjson`. Those were not parsed. They are configuration, not a live train feed.

## Seen in the booking bundle, not called

The Angular bundle at `/etc/designs/tickets/angular/main.*.js` builds two schedule URLs:

```
GET /dotcom/travel-service/schedules?origin={ORG}&destination={DST}&startDate={YYYY-MM-DD}
GET /dotcom/travel-service/schedules/routes/{route}/details?startdate={YYYY-MM-DD}&enddate={YYYY-MM-DD}
```

Those belong to the booking search. They likely overlap the official static GTFS. They were left uncalled.

The same bundle references signed-in paths (`/dotcom/order/retrieve`, `/dotcom/consumers/profile`, cart, and disruption types such as `SERVICE_CANCEL` / `SERVICE_DELAY` on a reservation). Those need a trip or an account. They were not requested.

`/services/contentService.iblegacy.stations.{code}.excludeheader.html` is an HTML fragment for a station blurb, not a status API.

## Polling, kept small

`no-store` on the status URLs means every call reaches origin. The map feed is the one fleet call. Everything under `www.amtrak.com` should stay on a single resident browser, the way PR #22 already does for tracks.

- **Positions.** Keep polling `getTrainsData` on the existing interval. Do not read position from `statusSummary.locationInfo`.
- **Delays and tracks for trains that are out.** The PR #22 window (a rail stop from 10 minutes past to 30 minutes ahead, a cap per cycle, about 60 seconds for the closest trains) matches how fresh the estimates were. Trip updates already carry estimated times from `getTrainsData`, so the value here is knowing which times are actuals, not the delay itself. Extending that same fetch to copy `dateTimeType` and `autoCalculated` is the integration with the least new traffic. Skip estimates that are `autoCalculated` and many stops away.
- **Gates.** A corridor board, not a per-train poll. Two or three pairs that publish gates, every few minutes, is a handful of requests per hour. NYP gate letters are the field the Lake Shore document did not have.
- **Cancellations and bus substitutions.** Take numbers from `/alert` detail pages. One status fetch per number and service date, then every 10–15 minutes. Cancellations were stable for weeks (`asOf` on 2 October for an 18 October trip). Emit the bus as its own trip only when `type.code` is `BUS`; the canceled train document does not name it.
- **Alerts.** Keep the HTML poll at 5–10 minutes. The JSON alert checks and the homepage banner can be ignored until a non-empty payload shows up.
- **Directories.** At most daily, or honor `max-age`. Never poll `/dotcom/stations/{code}/all` into logs.

Not recommended: walking every train number, every station pair, or the schedule search. That is a different traffic pattern from a rider opening Train Status a few times.

Every item above that reads `www.amtrak.com` is new automated traffic to Amtrak's site. Check it against Amtrak's terms of use before building it, as the `track-fetcher` README does for the track sidecar.
