import { describe, expect, it, vi } from 'vitest';
import { loadRootModule, type RootLoaders, type RootModule } from './mainRoot';

const appModule: RootModule = { default: () => null };
const panelModule: RootModule = { default: () => null };

function loaders() {
  return {
    app: vi.fn(async () => appModule),
    panel: vi.fn(async () => panelModule),
  } satisfies RootLoaders;
}

describe('loadRootModule', () => {
  it('loads only the dashboard module for the main window', async () => {
    const target = loaders();

    await expect(loadRootModule(false, target)).resolves.toBe(appModule);
    expect(target.app).toHaveBeenCalledOnce();
    expect(target.panel).not.toHaveBeenCalled();
  });

  it('loads only the lightweight tray module for the panel window', async () => {
    const target = loaders();

    await expect(loadRootModule(true, target)).resolves.toBe(panelModule);
    expect(target.panel).toHaveBeenCalledOnce();
    expect(target.app).not.toHaveBeenCalled();
  });
});
