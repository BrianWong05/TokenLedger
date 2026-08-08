import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import capability from '../src-tauri/capabilities/default.json';

describe('window drag capability', () => {
  it('allows Tauri drag regions to start moving the main window', () => {
    expect(capability.permissions).toContain('core:window:allow-start-dragging');
  });
});

// A modal's backdrop covers the shell's drag handles (sidebar, toolbars), so
// with a dialog open the only way left to move the frameless window is the
// dialog's own header.
describe('dialog headers are drag regions', () => {
  const HEADERS = [
    ['src/overview/TrendModal.tsx', 'tt-trend-modal-head'],
    ['src/overview/HeatmapModal.tsx', 'tt-heat-modal-head'],
    ['src/overview/CostBreakdownModal.tsx', 'tt-cost-modal-head'],
    ['src/pricing/OverrideEditor.tsx', 'tl-pr-dialog-head'],
  ];

  it.each(HEADERS)('%s marks .%s as a deep drag region', (file, className) => {
    const src = readFileSync(resolve(process.cwd(), file), 'utf8');
    expect(src).toMatch(new RegExp(`className="${className}"[^>]*data-tauri-drag-region="deep"`));
  });

  // The backdrop hides the shell's own handles, so it re-exposes the title-bar
  // row — where the window was always dragged from.
  it.each(HEADERS)('%s opens its backdrop with the drag strip', (file) => {
    const src = readFileSync(resolve(process.cwd(), file), 'utf8');
    expect(src).toMatch(/className="tl-modal-dragstrip"[^>]*data-tauri-drag-region/);
  });

  it('keeps the custom-range popover out of the drag region it renders inside', () => {
    const src = readFileSync(resolve(process.cwd(), 'src/overview/RangePicker.tsx'), 'utf8');
    expect(src).toMatch(/className="tt-dp"[^>]*data-tauri-drag-region="false"/);
  });
});
