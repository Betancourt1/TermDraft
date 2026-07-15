"""Shared terminal-native dialog framing."""

from textual.containers import Vertical


class TerminalDialog(Vertical):
    """Frame modal content with a centered title in its border."""

    def __init__(self, title: str, *, id: str) -> None:
        super().__init__(id=id, classes="dialog", markup=False)
        self.border_title = title
