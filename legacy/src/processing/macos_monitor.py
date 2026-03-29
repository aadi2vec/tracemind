import time
import threading
import subprocess
from typing import Optional, Tuple
from .ingest import Ingestor

class MacOSInteractionMonitor:
    """
    Passive observer for MacOS system interactions.
    Monitors active window titles and clipboard content.
    """
    def __init__(self, ingestor: Ingestor, interval: int = 15):
        self.ingestor = ingestor
        self.interval = interval
        self.running = False
        self.thread = None
        
        self.last_window: Optional[Tuple[str, str]] = None # (app, title)
        self.last_clipboard: Optional[str] = None

    def start(self):
        """Starts the passive monitoring thread."""
        self.running = True
        self.thread = threading.Thread(target=self._monitor_loop, daemon=True)
        self.thread.start()
        print(f"[MacOSMonitor] Started passive observation (interval={self.interval}s)...")

    def stop(self):
        self.running = False
        if self.thread:
            self.thread.join(timeout=1.0)
        print("[MacOSMonitor] Stopped.")

    def _get_active_window(self) -> Optional[Tuple[str, str]]:
        """Uses AppleScript to get the frontmost application and its window title."""
        script = """
        tell application "System Events"
            set frontApp to name of first application process whose frontmost is true
            tell process frontApp
                set windowTitle to name of front window
                return {frontApp, windowTitle}
            end tell
        end tell
        """
        try:
            output = subprocess.check_output(["osascript", "-e", script], text=True).strip()
            if "," in output:
                parts = [p.strip() for p in output.split(",")]
                return (parts[0], parts[1])
            return None
        except Exception:
            # Common failure if no window is open or permissions are lacking
            return None

    def _get_clipboard_content(self) -> Optional[str]:
        """Uses pbpaste to get the current clipboard text."""
        try:
            return subprocess.check_output(["pbpaste"], text=True).strip()
        except Exception:
            return None

    def _monitor_loop(self):
        while self.running:
            self.step()
            time.sleep(self.interval)

    def step(self):
        """Perform one polling iteration."""
        try:
            # 1. Check Active Window
            current_window = self._get_active_window()
            if current_window and current_window != self.last_window:
                app, title = current_window
                if title: # Only ingest if there's a title
                    msg = f"User is interacting with {app}: '{title}'"
                    print(f"\n[MacOSMonitor] Activity change: {msg}")
                    self.ingestor.ingest(msg, source="macos_window", app=app)
                self.last_window = current_window

            # 2. Check Clipboard
            current_clipboard = self._get_clipboard_content()
            if current_clipboard and current_clipboard != self.last_clipboard:
                # Only ingest if it's reasonably short but non-trivial
                if 10 < len(current_clipboard) < 1000:
                    msg = f"User copied text: '{current_clipboard}'"
                    print(f"\n[MacOSMonitor] Clipboard change detected.")
                    self.ingestor.ingest(msg, source="macos_clipboard")
                self.last_clipboard = current_clipboard

        except Exception as e:
            # Silently catch monitoring errors
            pass
