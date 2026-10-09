import { createRoot } from 'react-dom/client';
import { App } from './App.tsx';
import './styles.css';
import { applyTheme, readTheme } from './theme.ts';

applyTheme(readTheme());
const root = document.getElementById('root');
if (!root) throw new Error('缺少应用根元素');
createRoot(root).render(<App />);
