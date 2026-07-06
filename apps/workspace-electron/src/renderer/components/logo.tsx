// Biohazard VFX brand mark, from the studio-provided 07_Logos.zip.
// Two monochrome variants are swapped via CSS so the mark stays legible on either
// theme. Source luminance (measured): logo-dark is pure-black ink, logo-light is
// near-white ink — so the LIGHT logo renders in dark mode and the DARK logo in
// light mode. Purely class-driven (no JS); flips the instant the theme class does.

import logoDark from '@/assets/logo-dark.png';
import logoLight from '@/assets/logo-light.png';
import { cn } from '@/lib/utils';

export function Logo({ className }: { className?: string }) {
  return (
    <span className={cn('inline-flex items-center', className)} aria-label="Biohazard VFX">
      {/* dark ink → light mode */}
      <img src={logoDark} alt="" className="h-full w-auto dark:hidden" />
      {/* light ink → dark mode */}
      <img src={logoLight} alt="" className="hidden h-full w-auto dark:block" />
    </span>
  );
}
