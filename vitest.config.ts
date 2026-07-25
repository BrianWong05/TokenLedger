import { defaultExclude, defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    passWithNoTests: true,
    // nested worktrees hold stale copies of these same suites
    exclude: [...defaultExclude, '**/.claude/worktrees/**'],
  },
});
