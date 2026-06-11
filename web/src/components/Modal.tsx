import {
  ReactNode,
  RefObject,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  KeyboardEvent,
} from "react";
import { createPortal } from "react-dom";

// Module-level stack for ref-counted body scroll lock + z-index ordering.
// Each entry is the modal-id assigned via useId.
const modalStack: string[] = [];
let savedBodyOverflow: string | null = null;
let savedBodyPaddingRight: string | null = null;
// Codex HIGH v1: aria-hidden bookkeeping is owned by the stack, not by each modal.
// We snapshot background siblings' aria-hidden values on the FIRST push and restore
// them on the LAST pop, so a lower modal unmounting cannot expose the background
// while an upper modal remains.
let ariaHiddenRestore: Array<[Element, string | null]> | null = null;

function getScrollbarWidth(): number {
  if (typeof window === "undefined" || typeof document === "undefined") return 0;
  return window.innerWidth - document.documentElement.clientWidth;
}

function hideBackground(portalRoot: Element | null) {
  if (typeof document === "undefined") return;
  const root = document.body;
  const restore: Array<[Element, string | null]> = [];
  for (const el of Array.from(root.children)) {
    if (el === portalRoot) continue;
    restore.push([el, el.getAttribute("aria-hidden")]);
    el.setAttribute("aria-hidden", "true");
  }
  ariaHiddenRestore = restore;
}

function restoreBackground() {
  if (!ariaHiddenRestore) return;
  for (const [el, prev] of ariaHiddenRestore) {
    if (prev === null) el.removeAttribute("aria-hidden");
    else el.setAttribute("aria-hidden", prev);
  }
  ariaHiddenRestore = null;
}

function pushModal(id: string, portalRoot: Element | null) {
  if (modalStack.includes(id)) return;
  if (modalStack.length === 0 && typeof document !== "undefined") {
    const sbw = getScrollbarWidth();
    savedBodyOverflow = document.body.style.overflow;
    savedBodyPaddingRight = document.body.style.paddingRight;
    document.body.style.overflow = "hidden";
    if (sbw > 0) {
      const current = parseInt(getComputedStyle(document.body).paddingRight || "0", 10) || 0;
      document.body.style.paddingRight = `${current + sbw}px`;
    }
    hideBackground(portalRoot);
  }
  modalStack.push(id);
}

function popModal(id: string) {
  const idx = modalStack.lastIndexOf(id);
  if (idx === -1) return;
  modalStack.splice(idx, 1);
  if (modalStack.length === 0 && typeof document !== "undefined") {
    document.body.style.overflow = savedBodyOverflow ?? "";
    document.body.style.paddingRight = savedBodyPaddingRight ?? "";
    savedBodyOverflow = null;
    savedBodyPaddingRight = null;
    restoreBackground();
  }
}

function stackIndex(id: string): number {
  return modalStack.indexOf(id);
}

function isTopOfStack(id: string): boolean {
  return modalStack.length > 0 && modalStack[modalStack.length - 1] === id;
}

const FOCUSABLE_SELECTOR = [
  "button:not([disabled])",
  "[href]",
  "input:not([disabled]):not([type=\"hidden\"])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex=\"-1\"])",
  "audio[controls]",
  "video[controls]",
  "[contenteditable]:not([contenteditable=\"false\"])",
].join(",");

function getFocusable(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (el) => el.offsetParent !== null || el.getClientRects().length > 0,
  );
}

export interface ModalProps {
  title: string;
  subtitle?: ReactNode;
  ariaLabel?: string;
  onClose: () => void;
  onSubmit?: () => void;
  canSubmit?: boolean;
  maxWidth?: number | string;
  initialFocusRef?: RefObject<HTMLElement | null>;
  closeOnBackdrop?: boolean;
  showHeader?: boolean;
  children: ReactNode;
}

export function Modal({
  title,
  subtitle,
  ariaLabel,
  onClose,
  onSubmit,
  canSubmit,
  maxWidth = 640,
  initialFocusRef,
  closeOnBackdrop = true,
  showHeader = true,
  children,
}: ModalProps) {
  const modalId = useId();
  const titleId = `${modalId}-title`;
  const subtitleId = subtitle ? `${modalId}-subtitle` : undefined;
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const previouslyFocused = useRef<HTMLElement | null>(null);
  const [zIndex, setZIndex] = useState(1000);

  // Push/pop stack — manages body scroll lock + z-index + aria-hidden background.
  useLayoutEffect(() => {
    const portalRoot = dialogRef.current?.closest("[data-modal-portal]") ?? null;
    pushModal(modalId, portalRoot);
    setZIndex(1000 + stackIndex(modalId) * 10);
    return () => popModal(modalId);
  }, [modalId]);

  // Save + restore focus.
  useEffect(() => {
    if (typeof document === "undefined") return;
    previouslyFocused.current = document.activeElement as HTMLElement | null;
    return () => {
      const prev = previouslyFocused.current;
      // Defer to allow React to remove the dialog before refocusing the trigger.
      if (prev && typeof prev.focus === "function") {
        requestAnimationFrame(() => {
          // Codex MED v2: if another portal modal already owns focus (handoff
          // from a "close-and-open" pattern), don't yank it back.
          if (typeof document !== "undefined") {
            const active = document.activeElement as HTMLElement | null;
            if (active && active.closest("[data-modal-portal]")) return;
          }
          try {
            prev.focus({ preventScroll: true });
          } catch {
            // ignore
          }
        });
      }
    };
  }, []);

  // Initial focus: initialFocusRef → [autofocus] descendant → first non-close-button focusable.
  // Codex HIGH v1: skipping the close-× button matters because header renders before content,
  // so a naive "first focusable" steals focus from autoFocus textareas in Broadcast/Council/etc.
  useEffect(() => {
    if (initialFocusRef?.current) {
      initialFocusRef.current.focus({ preventScroll: true });
      return;
    }
    const dialog = dialogRef.current;
    if (!dialog) return;
    const autofocus = dialog.querySelector<HTMLElement>("[autofocus]");
    if (autofocus) {
      autofocus.focus({ preventScroll: true });
      return;
    }
    const focusables = getFocusable(dialog).filter(
      (el) => !el.hasAttribute("data-modal-close-button"),
    );
    const first = focusables[0] ?? dialog;
    first.focus({ preventScroll: true });
  }, [initialFocusRef]);

  const handleBackdropClick = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (e.target === e.currentTarget && closeOnBackdrop) {
        onClose();
      }
    },
    [closeOnBackdrop, onClose],
  );

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      // Only the top-most modal reacts to global shortcuts.
      if (!isTopOfStack(modalId)) return;
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        if (onSubmit && canSubmit !== false) {
          e.preventDefault();
          e.stopPropagation();
          onSubmit();
        }
        return;
      }
      if (e.key === "Tab") {
        // Focus trap — never interfere with bare keys (so Cmd+Tab / Alt+Tab system shortcuts are untouched).
        if (e.altKey || e.metaKey || e.ctrlKey) return;
        const dialog = dialogRef.current;
        if (!dialog) return;
        const focusables = getFocusable(dialog);
        if (focusables.length === 0) {
          e.preventDefault();
          return;
        }
        const first = focusables[0];
        const last = focusables[focusables.length - 1];
        const active = document.activeElement as HTMLElement | null;
        if (e.shiftKey) {
          if (active === first || !dialog.contains(active)) {
            e.preventDefault();
            last.focus();
          }
        } else {
          if (active === last) {
            e.preventDefault();
            first.focus();
          }
        }
      }
    },
    [canSubmit, modalId, onClose, onSubmit],
  );

  if (typeof document === "undefined") return null;

  const content = (
    <div
      className="modal-backdrop"
      onClick={handleBackdropClick}
      onKeyDown={handleKeyDown}
      data-modal-portal
      style={{ zIndex }}
    >
      <div
        ref={dialogRef}
        className="modal-dialog wizard"
        role="dialog"
        aria-modal="true"
        aria-label={ariaLabel ?? (typeof title === "string" ? title : undefined)}
        aria-labelledby={titleId}
        aria-describedby={subtitleId}
        tabIndex={-1}
        style={{ maxWidth }}
      >
        {showHeader && (
          <header className="wizard-header modal-header">
            <span className="hex" aria-hidden="true" />
            <div>
              <h2 id={titleId}>{title}</h2>
              {subtitle && <div className="muted" id={subtitleId}>{subtitle}</div>}
            </div>
            <button
              type="button"
              className="modal-close-x"
              onClick={onClose}
              aria-label="Close"
              title="Close (Esc)"
              data-modal-close-button="true"
            >
              ×
            </button>
          </header>
        )}
        <main className="wizard-body">{children}</main>
      </div>
    </div>
  );

  return createPortal(content, document.body);
}

export const __test__ = {
  modalStack,
  getScrollbarWidth,
};
