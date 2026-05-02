import { useEffect } from 'react';

type Handler = (e: KeyboardEvent) => void;

export function useGlobalKey(target: string | string[], handler: Handler): void {
  useEffect(() => {
    const targets = Array.isArray(target) ? target : [target];
    const onKey = (e: KeyboardEvent) => {
      if (targets.includes(e.key)) handler(e);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [target, handler]);
}
