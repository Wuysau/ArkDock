export type Theme = 'system' | 'light' | 'dark' | 'minimal-light' | 'titanium-dark';
export type ThemeTone = 'light' | 'dark' | 'system';
export type ThemeIcon = 'sun' | 'moon' | 'monitor' | 'spark';

export interface ThemeOption {
  id: Theme;
  label: string;
  englishName: string;
  description: string;
  tone: ThemeTone;
  icon: ThemeIcon;
  /** Preview palette: [canvas, card surface, accent]. */
  swatches: readonly [string, string, string];
}

export const themeOptions: ReadonlyArray<ThemeOption> = [
  {
    id: 'minimal-light',
    label: '极简浅色',
    englishName: 'Minimal Light',
    description: '纯净留白 · 柔和蓝调与磨砂卡片',
    tone: 'light',
    icon: 'sun',
    swatches: ['#F5F8FC', '#FFFFFF', '#3277F5'],
  },
  {
    id: 'titanium-dark',
    label: '深色钛金',
    englishName: 'Titanium Dark',
    description: '深邃石墨 · 冷银拉丝与环境微光',
    tone: 'dark',
    icon: 'spark',
    swatches: ['#10161D', '#212C39', '#5799FA'],
  },
  {
    id: 'dark',
    label: '深色',
    englishName: 'Classic Dark',
    description: '标准深海蓝玻璃外壳',
    tone: 'dark',
    icon: 'moon',
    swatches: ['#0B1220', '#17243A', '#4B82FF'],
  },
  {
    id: 'light',
    label: '浅色',
    englishName: 'Classic Light',
    description: '清爽浅蓝工作区外壳',
    tone: 'light',
    icon: 'sun',
    swatches: ['#E9EFF8', '#FFFFFF', '#275FD3'],
  },
  {
    id: 'system',
    label: '跟随系统',
    englishName: 'System',
    description: '随系统浅色或深色偏好自动切换',
    tone: 'system',
    icon: 'monitor',
    swatches: ['#10161D', '#F5F8FC', '#4B82FF'],
  },
];

const validThemes = new Set<string>(themeOptions.map((option) => option.id));

export function isTheme(value: unknown): value is Theme {
  return typeof value === 'string' && validThemes.has(value);
}

/** Invalid, empty, or unknown legacy storage values fall back safely to `'dark'`. */
export function parseTheme(value: string | null | undefined): Theme {
  return isTheme(value) ? value : 'dark';
}

export function readTheme(): Theme {
  try {
    return parseTheme(localStorage.getItem('arkdock:theme'));
  } catch {
    return 'dark';
  }
}

export function applyTheme(theme: Theme): void {
  const resolved = parseTheme(theme);
  document.documentElement.dataset.theme = resolved;
  try {
    localStorage.setItem('arkdock:theme', resolved);
  } catch { /* Theme still works without storage. */ }
}
