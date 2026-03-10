import time
import threading
import random
from typing import Callable
from .ingest import Ingestor

# Mock Data Source
MARKET_EVENTS = [
    "The Fed signals potentially higher rates for longer.",
    "Tech stocks rally on new AI chip announcements.",
    "Oil prices drop below $70 as supply increases.",
    "European Central Bank holds rates steady.",
    "Consumer spending verifies strong growth in Q3.",
    "Inflation data comes in hotter than expected.",
    "New trade tariffs announced on imported steel."
]

class NewsMonitor:
    def __init__(self, ingestor: Ingestor, interval: int = 30):
        self.ingestor = ingestor
        self.interval = interval
        self.running = False
        self.thread = None

    def start(self):
        """Starts the background monitoring thread."""
        self.running = True
        self.thread = threading.Thread(target=self._monitor_loop, daemon=True)
        self.thread.start()
        print(f"[Monitor] Started monitoring news sources every {self.interval}s...")

    def stop(self):
        self.running = False
        if self.thread:
            self.thread.join(timeout=1.0)
        print("[Monitor] Stopped.")

    def _monitor_loop(self):
        while self.running:
            # Simulate fetching news
            event = random.choice(MARKET_EVENTS)
            timestamp = time.strftime("%H:%M:%S")
            print(f"\n[Monitor {timestamp}] Discovered new info: '{event}'")
            
            # Ingest
            try:
                print(f"[Monitor] Ingesting...")
                self.ingestor.ingest(event, source="news_stream")
                print(f"[Monitor] Knowledge Graph Updated.")
            except Exception as e:
                print(f"[Monitor] Ingestion failed: {e}")
            
            # Wait for next interval
            time.sleep(self.interval)
