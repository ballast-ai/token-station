import { createContext, useCallback, useContext, useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useLocalizedCopy } from "./LanguageProvider";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "./ui/alert-dialog";

type DraftNavigation = {
  register: (id: string, dirty: boolean) => void;
  navigate: (action: () => void) => void;
};
const DraftContext = createContext<DraftNavigation | null>(null);
function DraftNavigationProvider({ children }: { children: ReactNode }) {
  const { copy } = useLocalizedCopy();
  const dirtyIds = useRef(new Set<string>());
  const origin = useRef<HTMLElement | null>(null);
  const pending = useRef<(() => void) | null>(null);
  const [open, setOpen] = useState(false);
  const register = useCallback((id: string, dirty: boolean) => {
    if (dirty) dirtyIds.current.add(id);
    else dirtyIds.current.delete(id);
  }, []);
  const navigate = useCallback((action: () => void) => {
    if (dirtyIds.current.size === 0) { action(); return; }
    origin.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    pending.current = action;
    setOpen(true);
  }, []);
  return <DraftContext.Provider value={{ register, navigate }}>
    {children}
    <AlertDialog open={open} onOpenChange={setOpen}>
      <AlertDialogContent onCloseAutoFocus={(event) => {
        event.preventDefault();
        if (origin.current?.isConnected) origin.current.focus();
      }}>
        <AlertDialogHeader>
          <AlertDialogTitle>{copy("Unsaved changes", "有未保存更改", "有未儲存的變更", "未保存の変更があります")}</AlertDialogTitle>
          <AlertDialogDescription>{copy("Leaving will discard this form's unsaved changes.", "离开会放弃当前表单中尚未保存的更改。", "離開會放棄目前表單中尚未儲存的變更。", "移動すると、このフォームの未保存の変更は破棄されます。")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{copy("Continue editing", "继续编辑", "繼續編輯", "編集を続ける")}</AlertDialogCancel>
          <AlertDialogAction onClick={() => {
            const action = pending.current;
            pending.current = null;
            origin.current = null;
            setOpen(false);
            action?.();
          }}>{copy("Discard changes and leave", "放弃更改并离开", "放棄變更並離開", "変更を破棄して移動")}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  </DraftContext.Provider>;
}
export function DraftNavigationBoundary({ children }: { children: ReactNode }) {
  const existing = useContext(DraftContext);
  return existing ? children : <DraftNavigationProvider>{children}</DraftNavigationProvider>;
}
export function useDraftGuard(dirty: boolean) {
  const register = useContext(DraftContext)?.register;
  const id = useId();
  useEffect(() => {
    register?.(id, dirty);
    return () => register?.(id, false);
  }, [dirty, id, register]);
}
export function useDraftNavigation() {
  const context = useContext(DraftContext);
  return context?.navigate ?? ((action: () => void) => action());
}
