import { forwardRef, useRef } from 'react';
import type { PreviewKey } from '../ipc/model.ts';

const keys: Record<string, PreviewKey> = { Enter: 'enter', Backspace: 'backspace', Delete: 'delete',
  Escape: 'escape', ArrowUp: 'arrowUp', ArrowDown: 'arrowDown', ArrowLeft: 'arrowLeft', ArrowRight: 'arrowRight', Home: 'home', End: 'end' };

/** Only this focused, local input receives committed text. Preedit/candidate transactions stay in the Windows IME. */
export const NativeTextCapture = forwardRef<HTMLTextAreaElement, {
  enabled: boolean; commit: (text: string) => void; keyPress: (key: PreviewKey) => void; blur: () => void;
}>(function NativeTextCapture({ enabled, commit, keyPress, blur }, ref) {
  const composing = useRef(false);
  const finalComposition = useRef<string | null>(null);
  function flush(element: HTMLTextAreaElement) {
    const text = element.value; element.value = '';
    if (!enabled || !document.hasFocus() || document.activeElement !== element || !text) return;
    if (finalComposition.current === text) { finalComposition.current = null; return; }
    finalComposition.current = null; commit(text);
  }
  return <textarea ref={ref} className="native-text-capture" aria-label="向当前鸿蒙输入焦点提交文字"
    disabled={!enabled} tabIndex={-1} autoCapitalize="off" autoComplete="off" spellCheck={false}
    onCompositionStart={(event) => { composing.current = enabled && document.hasFocus() && document.activeElement === event.currentTarget; finalComposition.current = null; }}
    onCompositionEnd={(event) => {
      composing.current = false;
      const text = event.data;
      event.currentTarget.value = '';
      if (enabled && document.hasFocus() && document.activeElement === event.currentTarget && text) { finalComposition.current = text; commit(text); }
    }}
    onInput={(event) => { if (!composing.current && !(event.nativeEvent as InputEvent).isComposing) flush(event.currentTarget); }}
    onKeyDown={(event) => {
      if (!enabled || !document.hasFocus() || document.activeElement !== event.currentTarget || composing.current || event.nativeEvent.isComposing || event.keyCode === 229 || event.ctrlKey || event.metaKey || event.altKey) return;
      finalComposition.current = null;
      const key = keys[event.key];
      if (!key) return;
      event.preventDefault(); keyPress(key);
    }} onPaste={() => { finalComposition.current = null; }}
    onBlur={(event) => { composing.current = false; finalComposition.current = null; event.currentTarget.value = ''; blur(); }} />;
});
