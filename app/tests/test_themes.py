#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later

import pathlib
import unittest


APP_DIR = pathlib.Path(__file__).resolve().parents[1]
PROJECT = APP_DIR.parent
THEME_DIR = APP_DIR / "Lomiri/Components/Themes/PuhuriSininen"


class BlueBlackThemeTest(unittest.TestCase):
    def test_theme_mapping_contains_the_third_theme(self):
        theme_js = (APP_DIR / "qml/theme.js").read_text()
        self.assertIn(
            '{ key: "sininen", module: "Lomiri.Components.Themes.PuhuriSininen" }',
            theme_js,
        )

    def test_settings_offer_and_select_the_third_theme(self):
        settings = (APP_DIR / "qml/SettingsPage.qml").read_text()
        self.assertIn('tr("settings.themeBlue")', settings)
        self.assertIn('"sininen"', settings)

    def test_both_languages_name_the_theme(self):
        i18n = (APP_DIR / "qml/i18n.js").read_text()
        self.assertEqual(i18n.count('"settings.themeBlue":'), 2)
        self.assertIn('"settings.themeBlue": "Revontuli blue-black"', i18n)
        self.assertIn('"settings.themeBlue": "Revontuli sinimusta"', i18n)

    def test_bundled_module_contains_the_blue_black_palette(self):
        normal = (
            THEME_DIR / "1.3/PuhuriSininenNormal.qml"
        ).read_text()
        selected = (
            THEME_DIR / "1.3/PuhuriSininenSelected.qml"
        ).read_text()
        self.assertEqual((THEME_DIR / "parent_theme").read_text().strip(),
                         "Lomiri.Components.Themes.PuhuriPerus")
        self.assertIn('background: "#000000"', normal)
        self.assertIn('baseText: "#4A9EFF"', normal)
        self.assertIn('background: "#4A9EFF"', selected)
        self.assertIn('baseText: "#000000"', selected)

    def test_normal_text_fields_are_lighter_black_with_blue_text(self):
        normal = (
            THEME_DIR / "1.3/PuhuriSininenNormal.qml"
        ).read_text()
        self.assertIn('field: "#151515"', normal)
        self.assertIn('fieldText: "#4A9EFF"', normal)

    def test_12_palette_has_lighter_black_normal_text_fields(self):
        palette = (THEME_DIR / "1.2/Palette.qml").read_text()
        normal = palette.split("normal: PaletteValues {", 1)[1].split(
            "\n    }", 1
        )[0]
        self.assertIn('field: "#151515"', normal)
        self.assertIn('fieldText: "#4A9EFF"', normal)

    def test_sync_script_reproduces_the_third_module(self):
        sync = (PROJECT / "tools/sync-teemat.sh").read_text()
        self.assertIn(
            "copy_theme RevontuliMustavalkoinenSininen "
            "PuhuriSininen RevontuliMustavalkoinenSininen PuhuriSininen",
            sync,
        )
        self.assertIn(
            'echo "Lomiri.Components.Themes.PuhuriPerus" '
            '>"$TARGET/PuhuriSininen/parent_theme"',
            sync,
        )


if __name__ == "__main__":
    unittest.main()
