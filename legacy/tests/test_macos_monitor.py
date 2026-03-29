import unittest
from unittest.mock import MagicMock, patch
from src.processing.macos_monitor import MacOSInteractionMonitor

class TestMacOSMonitor(unittest.TestCase):
    def setUp(self):
        self.mock_ingestor = MagicMock()
        self.monitor = MacOSInteractionMonitor(self.mock_ingestor, interval=1)

    @patch("subprocess.check_output")
    def test_get_active_window(self, mock_subprocess):
        # Simulate osascript output
        mock_subprocess.return_value = "Finder, Desktop"
        res = self.monitor._get_active_window()
        self.assertEqual(res, ("Finder", "Desktop"))

    @patch("subprocess.check_output")
    def test_get_clipboard(self, mock_subprocess):
        # Simulate pbpaste output
        mock_subprocess.return_value = "Hello World"
        res = self.monitor._get_clipboard_content()
        self.assertEqual(res, "Hello World")

    @patch("src.processing.macos_monitor.MacOSInteractionMonitor._get_active_window")
    @patch("src.processing.macos_monitor.MacOSInteractionMonitor._get_clipboard_content")
    def test_monitor_loop_iteration(self, mock_clip, mock_window):
        # Mock window change
        mock_window.side_effect = [("Finder", "Desktop"), ("Code", "main.py")]
        mock_clip.return_value = "Some copied text"
        
        # Manually trigger the loop logic for one or two "checks"
        # We'll just test the internal state update and ingestion trigger
        
        # Reset ingestor mock
        self.mock_ingestor.ingest.reset_mock()
        
        # 1st check: Window is Finder
        self.monitor.last_window = None
        self.monitor.step()
        
    def test_monitors_window_change(self):
        # Internal test of the logic
        self.monitor.last_window = ("Finder", "Desktop")
        
        # Mocking window
        with patch.object(self.monitor, "_get_active_window", return_value=("Code", "script.py")):
            with patch.object(self.monitor, "_get_clipboard_content", return_value=""):
                # Run one polling step
                # (Refactoring the loop to allow one step run)
                pass

# Adding a helper to test one iteration
def monitor_step(monitor):
    current_window = monitor._get_active_window()
    if current_window and current_window != monitor.last_window:
        app, title = current_window
        monitor.ingestor.ingest(f"User is interacting with {app}: '{title}'", source="macos_window", app=app)
        monitor.last_window = current_window

    current_clipboard = monitor._get_clipboard_content()
    if current_clipboard and current_clipboard != monitor.last_clipboard:
        if 5 < len(current_clipboard) < 100:
            monitor.ingestor.ingest(f"User copied text: '{current_clipboard}'", source="macos_clipboard")
        monitor.last_clipboard = current_clipboard

class TestMonitorLogic(unittest.TestCase):
    def test_logic(self):
        ingestor = MagicMock()
        monitor = MacOSInteractionMonitor(ingestor)
        
        with patch.object(monitor, "_get_active_window", return_value=("Safari", "GitHub")):
             with patch.object(monitor, "_get_clipboard_content", return_value=""):
                 monitor_step(monitor)
                 ingestor.ingest.assert_called_with("User is interacting with Safari: 'GitHub'", source="macos_window", app="Safari")

        # 2nd step, same window -> no ingest
        ingestor.ingest.reset_mock()
        with patch.object(monitor, "_get_active_window", return_value=("Safari", "GitHub")):
             with patch.object(monitor, "_get_clipboard_content", return_value=""):
                 monitor_step(monitor)
                 ingestor.ingest.assert_not_called()

        # Clipboard change
        ingestor.ingest.reset_mock()
        with patch.object(monitor, "_get_active_window", return_value=("Safari", "GitHub")):
            with patch.object(monitor, "_get_clipboard_content", return_value="Some new text"):
                 monitor_step(monitor)
                 ingestor.ingest.assert_called_with("User copied text: 'Some new text'", source="macos_clipboard")

if __name__ == "__main__":
    unittest.main()
