import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

// Tailwind-aware className combiner. clsx handles conditional/arrays;
// twMerge dedupes conflicting Tailwind utilities so later classes win.
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
