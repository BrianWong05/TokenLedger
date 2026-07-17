import { describe, expect, it } from 'vitest';
import capability from '../src-tauri/capabilities/default.json';

describe('window drag capability', () => {
  it('allows Tauri drag regions to start moving the main window', () => {
    expect(capability.permissions).toContain('core:window:allow-start-dragging');
  });
});
