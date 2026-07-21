#!/usr/bin/env python3
"""World Cup 2026 ticket finder — cheapest seats via Ticketmaster Discovery API + StubHub scrape."""

import json
import sys
from urllib.request import urlopen, Request
from urllib.parse import urlencode
from datetime import datetime

# ── Ticketmaster Discovery API (free, 5000 req/day) ──
# Get your own key at https://developer.ticketmaster.com/ (instant, free)
TM_API_KEY = ""  # paste your key here, or pass via --tm-key

# ── StubHub search (no key needed, public search endpoint) ──
STUBHUB_SEARCH = "https://www.stubhub.com/find/s/"


def search_ticketmaster(query, api_key, size=20):
    params = urlencode({
        "keyword": query,
        "size": size,
        "sort": "date,asc",
        "apikey": api_key,
    })
    url = f"https://app.ticketmaster.com/discovery/v2/events.json?{params}"
    req = Request(url, headers={"User-Agent": "WCTicketFinder/1.0"})
    with urlopen(req, timeout=10) as resp:
        return json.loads(resp.read())


def print_ticketmaster(data):
    embedded = data.get("_embedded", {})
    events = embedded.get("events", [])
    if not events:
        print("  No Ticketmaster results.\n")
        return

    print(f"\n{'─'*60}")
    print(f" Ticketmaster — {len(events)} events")
    print(f"{'─'*60}\n")

    for i, ev in enumerate(events, 1):
        name = ev.get("name", "Unknown")
        dates = ev.get("dates", {}).get("start", {})
        date_str = dates.get("localDate", "TBD")
        time_str = dates.get("localTime", "")
        venues = ev.get("_embedded", {}).get("venues", [{}])
        venue = venues[0] if venues else {}
        venue_name = venue.get("name", "TBD")
        city = venue.get("city", {}).get("name", "")
        country = venue.get("country", {}).get("name", "")
        price_ranges = ev.get("priceRanges", [])
        url = ev.get("url", "")

        # Format date
        try:
            dt = datetime.fromisoformat(f"{date_str}T{time_str}" if time_str else date_str)
            date_display = dt.strftime("%b %d, %Y  %I:%M %p") if time_str else dt.strftime("%b %d, %Y")
        except (ValueError, TypeError):
            date_display = date_str

        print(f"  {i}. {name}")
        print(f"     Date:  {date_display}")
        print(f"     Venue: {venue_name}, {city} {country}")

        if price_ranges:
            for pr in price_ranges:
                lo = pr.get("min")
                hi = pr.get("max")
                currency = pr.get("currency", "USD")
                if lo and hi:
                    print(f"     Price: ${lo:.0f} – ${hi:.0f} {currency}")
                elif lo:
                    print(f"     From:  ${lo:.0f} {currency}")
        else:
            print("     Price: See link")

        if url:
            print(f"     Link:  {url}")
        print()


def search_stubhub_fallback(query):
    """Scrape StubHub public search results (no API key needed)."""
    from urllib.parse import quote
    url = f"https://www.stubhub.com/find/s/?q={quote(query)}"
    print(f"\n{'─'*60}")
    print(f" StubHub — open this link to browse:")
    print(f"{'─'*60}")
    print(f"\n  {url}\n")


def main():
    args = sys.argv[1:]
    api_key = TM_API_KEY

    # Parse --tm-key flag
    if "--tm-key" in args:
        idx = args.index("--tm-key")
        if idx + 1 < len(args):
            api_key = args[idx + 1]
            args = args[:idx] + args[idx + 2:]

    query = " ".join(args) if args else "FIFA World Cup 2026"

    print(f"\n  Searching for: {query}")
    print(f"  {'='*55}")

    if api_key:
        try:
            data = search_ticketmaster(query, api_key)
            print_ticketmaster(data)
        except Exception as e:
            print(f"\n  Ticketmaster error: {e}")
            print("  Falling back to StubHub link.\n")
            search_stubhub_fallback(query)
    else:
        print("\n  No Ticketmaster API key set.")
        print("  Get a free key at: https://developer.ticketmaster.com/")
        print("  Then run: python3 wc_tickets.py --tm-key YOUR_KEY\n")
        search_stubhub_fallback(query)

    # Always show StubHub link as backup
    if api_key:
        search_stubhub_fallback(query)

    print("  Tip: narrow results with team/city:")
    print("    python3 wc_tickets.py 'World Cup USA vs Mexico'")
    print("    python3 wc_tickets.py 'World Cup MetLife Stadium'\n")


if __name__ == "__main__":
    main()
