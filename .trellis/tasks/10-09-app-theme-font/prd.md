# RCH app theme and font settings

## Goal

Give readers a few polished, readable app-wide color palettes and a separate font preference, with the same settings on mobile and desktop.

## User requirements

- Keep the existing light and dark choices.
- Add fixed palette presets: Classic, Sea Glass, Warm Paper, and Ink Night.
- Keep brightness and palette as separate choices. Ink Night uses near-black surfaces in dark mode; its light variant remains a neutral light palette.
- Add two app UI font choices: System Default and System Serif.
- Apply these choices across the entire app, including the library, recent reading, statistics, tags, source browser, settings, and reader controls.
- Keep the actual comic image/canvas appearance independently controlled by reader settings.
- Present controls with enough spacing on phones and desktop so the appearance settings remain easy to scan.

## Constraints

- Preserve the existing default: dark mode with the Classic palette and System Default font.
- Persist preferences using the existing local app settings mechanism; old settings files must load with the defaults above.
- Do not add bundled or downloaded fonts in this phase. Native serif rendering can vary by platform.
- Keep status colors for errors, warnings, and success meaningful and readable; use theme roles for general surfaces, text, borders, and selection states.
- Normal text must meet WCAG 2.1 AA contrast of at least 4.5:1 against its background. Large text and non-text controls must meet their applicable contrast requirements.

## Acceptance criteria

- [ ] A user can select light/dark brightness, one of the four fixed palettes, and either font independently.
- [ ] The selected appearance updates immediately and persists after restarting the app.
- [ ] Existing settings without palette/font keys continue to load as dark + Classic + System Default.
- [ ] Palette and font settings apply to mobile and desktop routes throughout the app; comic image pixels are unchanged.
- [ ] Ink Night has near-black dark-mode surfaces with legible text; all other palette/mode combinations preserve readable foreground/background pairs.
- [ ] Generic UI text and surfaces follow the selected theme. Semantic status colors and the reader canvas retain their intended independent meanings.
- [ ] Appearance controls fit compact screens and wider desktop layouts without clipping or crowding.

## Out of scope

- User-defined colors, wallpaper/background images, system dynamic colors, downloadable fonts, and bundled CJK font files.
- Changing reader canvas/background preferences or applying the app font to text embedded in comic images.
