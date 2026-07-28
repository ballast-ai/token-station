import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/** shadcn/ui class-name helper: merge conditional classes, de-duplicating
 *  conflicting Tailwind utilities (the later one wins). */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
