export type Theme = 'system' | 'light' | 'dark';
export function readTheme(): Theme {
  try {
    const value = localStorage.getItem('arkdock:theme');
    return value === 'light' || value === 'dark' || value === 'system' ? value : 'dark';
  } catch { return 'dark'; }
}
export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
  try { localStorage.setItem('arkdock:theme', theme); } catch { /* Theme still works without storage. */ }
}
