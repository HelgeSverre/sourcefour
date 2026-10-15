#!/usr/bin/env python3
"""Static contracts for the Windows MSI definition.

The real installer is built on Windows. These checks keep the Start Menu and
Apps & Features icon bindings covered by the normal cross-platform test gate.
"""

from pathlib import Path, PureWindowsPath
import unittest
import xml.etree.ElementTree as ElementTree


ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "apps" / "sourcefour"
WIX = PACKAGE / "wix" / "main.wxs"
NAMESPACE = {"wix": "http://schemas.microsoft.com/wix/2006/wi"}


class WindowsDistributionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = ElementTree.parse(WIX)

    def test_installed_surfaces_use_the_packaged_icon(self):
        icon = self.document.find(".//wix:Icon[@Id='ProductICO']", NAMESPACE)
        shortcut = self.document.find(
            ".//wix:Shortcut[@Id='ApplicationStartMenuShortcut']", NAMESPACE
        )
        product_icon = self.document.find(
            ".//wix:Property[@Id='ARPPRODUCTICON']", NAMESPACE
        )

        self.assertIsNotNone(icon)
        self.assertIsNotNone(shortcut)
        self.assertIsNotNone(product_icon)
        self.assertEqual(shortcut.get("Icon"), icon.get("Id"))
        self.assertEqual(product_icon.get("Value"), icon.get("Id"))

        source = PureWindowsPath(icon.get("SourceFile"))
        icon_path = PACKAGE.joinpath(*source.parts)
        self.assertTrue(icon_path.is_file(), f"missing packaged icon: {icon_path}")
        self.assertEqual(icon_path.read_bytes()[:4], b"\x00\x00\x01\x00")


if __name__ == "__main__":
    unittest.main(verbosity=2)
