type IconName = 'preview' | 'device' | 'folder' | 'log' | 'settings' | 'link' | 'refresh' | 'rotate' | 'spark' | 'shield' | 'close' | 'chevron' | 'search' | 'moon' | 'sun' | 'monitor' | 'grip' | 'stop' | 'arrow' | 'maximize' | 'restore';

const paths: Record<IconName, string> = {
  preview: 'M8 2.5h8a2 2 0 0 1 2 2v15a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2v-15a2 2 0 0 1 2-2ZM10 6h4M11 18h2',
  device: 'M7 2.5h10a2 2 0 0 1 2 2v15a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2v-15a2 2 0 0 1 2-2ZM10 3v2h4V3M11 18h2',
  folder: 'M3 7V5a2 2 0 0 1 2-2h4l2 3h8a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7ZM3 8h18M8 12v4M12 12v4M16 12v4',
  log: 'M7 3h10a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2ZM9 8h6M9 12h6M9 16h4',
  settings: 'M9 3h6l.7 3 2.6 1.5 2.8-.9 3 5.2-2.2 2.1v3l2.2 2.1-3 5.2-2.8-.9L15.7 22l-.7 3H9l-.7-3-2.6-1.5-2.8.9-3-5.2 2.2-2.1v-3L-.1 9l3-5.2 2.8.9L8.3 3Z',
  link: 'M9 15l6-6M8 16H6a4 4 0 0 1 0-8h4M16 8h2a4 4 0 0 1 0 8h-4',
  refresh: 'M20 8a8 8 0 1 0 0 8M20 3v5h-5',
  rotate: 'M4 7a9 9 0 0 1 15-2l2 3M21 3v5h-5M7 9h7v11H7ZM10 17h1',
  spark: 'm12 2 2.5 7.5L22 12l-7.5 2.5L12 22l-2.5-7.5L2 12l7.5-2.5L12 2Z',
  shield: 'm12 2 8 3v7c0 5-8 10-8 10S4 17 4 12V5l8-3ZM8.5 12l2.5 2.5 4.5-5',
  close: 'm6 6 12 12M6 18 18 6',
  chevron: 'm9 5 7 7-7 7',
  search: 'M10.5 3a7.5 7.5 0 1 0 0 15 7.5 7.5 0 0 0 0-15ZM16 16l5 5',
  moon: 'M20 14a8 8 0 0 1-10-10 9 9 0 1 0 10 10Z',
  sun: 'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5 19 19M5 19l1.5-1.5M17.5 6.5 19 5',
  monitor: 'M4 3h16a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1ZM8 21h8M12 17v4',
  grip: 'M8 5h.01M16 5h.01M8 12h.01M16 12h.01M8 19h.01M16 19h.01',
  stop: 'M6 6h12v12H6Z',
  arrow: 'M4 12h16m-6-6 6 6-6 6',
  maximize: 'M5 5h14v14H5Z',
  restore: 'M8 8h11v11H8ZM5 16H3V3h13v2',
};

export function Icon({ name, size = 20 }: { name: IconName; size?: number }) {
  if (name === 'settings') return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m9 3-1 3-3 1-2 5 2 5 3 1 1 3h6l1-3 3-1 2-5-2-5-3-1-1-3Z" /><circle cx="12" cy="12" r="3" /></svg>;
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={name === 'grip' ? 3.5 : 1.65} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[name]} /></svg>;
}
