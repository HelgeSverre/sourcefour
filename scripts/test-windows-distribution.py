#!/usr/bin/env python3
"""Static contracts for the Windows MSI definition.

The real installer is built on Windows. These checks keep the Start Menu and
Apps & Features icon bindings covered by the normal cross-platform test gate.
"""

from pathlib import Path, PureWindowsPath
import re
import unittest
import xml.etree.ElementTree as ElementTree


ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "apps" / "sourcefour"
WIX = PACKAGE / "wix" / "main.wxs"
RESOURCE = PACKAGE / "windows.rc"
NAMESPACE = {"wix": "http://schemas.microsoft.com/wix/2006/wi"}


class WindowsDistributionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = ElementTree.parse(WIX)

    def test_installed_surfaces_use_the_packaged_icon(self):
        shortcut = self.document.find(
            ".//wix:File/wix:Shortcut[@Id='ApplicationStartMenuShortcut']", NAMESPACE
        )
        product_icon = self.document.find(
            ".//wix:Property[@Id='ARPPRODUCTICON']", NAMESPACE
        )

        self.assertIsNotNone(shortcut)
        self.assertIsNotNone(product_icon)
        arp_icon = self.document.find(
            f".//wix:Icon[@Id='{product_icon.get('Value')}']", NAMESPACE
        )
        self.assertIsNotNone(arp_icon)
        self.assertTrue(arp_icon.get("Id").endswith(".ico"))
        self.assertEqual(shortcut.get("Advertise"), "no")
        self.assertIsNone(shortcut.get("Target"))
        self.assertIsNone(shortcut.get("Icon"))
        self.assertIsNone(shortcut.get("IconIndex"))

        resource_match = re.fullmatch(
            r'\s*\d+\s+ICON\s+"([^"]+)"\s*', RESOURCE.read_text()
        )
        self.assertIsNotNone(resource_match)
        executable_icon = PACKAGE / resource_match.group(1)

        arp_source = PureWindowsPath(arp_icon.get("SourceFile"))
        arp_icon_path = PACKAGE.joinpath(*arp_source.parts)
        self.assertEqual(executable_icon.read_bytes(), arp_icon_path.read_bytes())
        self.assertEqual(executable_icon.read_bytes()[:4], b"\x00\x00\x01\x00")


if __name__ == "__main__":
    unittest.main(verbosity=2)
