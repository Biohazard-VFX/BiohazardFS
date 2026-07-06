import {
  Box,
  Braces,
  File,
  FileArchive,
  FileAudio,
  FileCode,
  FileImage,
  FileText,
  FileVideo,
  Palette,
  type LucideIcon,
} from 'lucide-react';

import { cn } from '@/lib/utils';

// Per-extension file icons for VFX / video-editing workflows. Color is
// categorical (one hue per media family), not decorative — each tint means a
// kind of asset, the way Finder or a media tool conveys type at a glance.
// Hue assignments are deliberately small and stable across light/dark.

type Kind = { icon: LucideIcon; tint: string };

const KINDS: Record<string, Kind> = {
  // Image / plates / textures
  exr: { icon: FileImage, tint: 'text-amber-500' },
  dpx: { icon: FileImage, tint: 'text-amber-500' },
  cin: { icon: FileImage, tint: 'text-amber-500' },
  tif: { icon: FileImage, tint: 'text-amber-500' },
  tiff: { icon: FileImage, tint: 'text-amber-500' },
  png: { icon: FileImage, tint: 'text-amber-500' },
  jpg: { icon: FileImage, tint: 'text-amber-500' },
  jpeg: { icon: FileImage, tint: 'text-amber-500' },
  webp: { icon: FileImage, tint: 'text-amber-500' },
  hdr: { icon: FileImage, tint: 'text-amber-500' },
  // Video / playables
  mov: { icon: FileVideo, tint: 'text-sky-500' },
  mp4: { icon: FileVideo, tint: 'text-sky-500' },
  m4v: { icon: FileVideo, tint: 'text-sky-500' },
  mxf: { icon: FileVideo, tint: 'text-sky-500' },
  avi: { icon: FileVideo, tint: 'text-sky-500' },
  mkv: { icon: FileVideo, tint: 'text-sky-500' },
  webm: { icon: FileVideo, tint: 'text-sky-500' },
  // 3D / scene / interchange / cache
  hip: { icon: Box, tint: 'text-emerald-500' },
  hipnc: { icon: Box, tint: 'text-emerald-500' },
  hiplc: { icon: Box, tint: 'text-emerald-500' },
  hiplt: { icon: Box, tint: 'text-emerald-500' },
  blend: { icon: Box, tint: 'text-emerald-500' },
  blend1: { icon: Box, tint: 'text-emerald-500' },
  ma: { icon: Box, tint: 'text-emerald-500' },
  mb: { icon: Box, tint: 'text-emerald-500' },
  abc: { icon: Box, tint: 'text-emerald-500' },
  usd: { icon: Box, tint: 'text-emerald-500' },
  usda: { icon: Box, tint: 'text-emerald-500' },
  usdc: { icon: Box, tint: 'text-emerald-500' },
  usdz: { icon: Box, tint: 'text-emerald-500' },
  fbx: { icon: Box, tint: 'text-emerald-500' },
  obj: { icon: Box, tint: 'text-emerald-500' },
  glb: { icon: Box, tint: 'text-emerald-500' },
  gltf: { icon: Box, tint: 'text-emerald-500' },
  // Compositing / scripts
  nk: { icon: FileCode, tint: 'text-blue-500' },
  nknc: { icon: FileCode, tint: 'text-blue-500' },
  py: { icon: FileCode, tint: 'text-blue-500' },
  sh: { icon: FileCode, tint: 'text-blue-500' },
  bash: { icon: FileCode, tint: 'text-blue-500' },
  vex: { icon: FileCode, tint: 'text-blue-500' },
  mel: { icon: FileCode, tint: 'text-blue-500' },
  js: { icon: FileCode, tint: 'text-blue-500' },
  ts: { icon: FileCode, tint: 'text-blue-500' },
  // LUT / color science
  cube: { icon: Palette, tint: 'text-rose-500' },
  '3dl': { icon: Palette, tint: 'text-rose-500' },
  lut: { icon: Palette, tint: 'text-rose-500' },
  blut: { icon: Palette, tint: 'text-rose-500' },
  csp: { icon: Palette, tint: 'text-rose-500' },
  spi1d: { icon: Palette, tint: 'text-rose-500' },
  spi3d: { icon: Palette, tint: 'text-rose-500' },
  vf: { icon: Palette, tint: 'text-rose-500' },
  // Audio
  wav: { icon: FileAudio, tint: 'text-violet-500' },
  aiff: { icon: FileAudio, tint: 'text-violet-500' },
  aif: { icon: FileAudio, tint: 'text-violet-500' },
  flac: { icon: FileAudio, tint: 'text-violet-500' },
  mp3: { icon: FileAudio, tint: 'text-violet-500' },
  aac: { icon: FileAudio, tint: 'text-violet-500' },
  ogg: { icon: FileAudio, tint: 'text-violet-500' },
  // Data / config
  json: { icon: Braces, tint: 'text-muted-foreground' },
  xml: { icon: Braces, tint: 'text-muted-foreground' },
  yaml: { icon: Braces, tint: 'text-muted-foreground' },
  yml: { icon: Braces, tint: 'text-muted-foreground' },
  toml: { icon: Braces, tint: 'text-muted-foreground' },
  ini: { icon: Braces, tint: 'text-muted-foreground' },
  csv: { icon: Braces, tint: 'text-muted-foreground' },
  // Docs / text
  txt: { icon: FileText, tint: 'text-muted-foreground' },
  md: { icon: FileText, tint: 'text-muted-foreground' },
  pdf: { icon: FileText, tint: 'text-muted-foreground' },
  rtf: { icon: FileText, tint: 'text-muted-foreground' },
  // Archives
  zip: { icon: FileArchive, tint: 'text-muted-foreground' },
  tar: { icon: FileArchive, tint: 'text-muted-foreground' },
  gz: { icon: FileArchive, tint: 'text-muted-foreground' },
  '7z': { icon: FileArchive, tint: 'text-muted-foreground' },
  rar: { icon: FileArchive, tint: 'text-muted-foreground' },
};

const DEFAULT_KIND: Kind = { icon: File, tint: 'text-muted-foreground' };

function extOf(name: string): string {
  const dot = name.lastIndexOf('.');
  if (dot < 0 || dot === name.length - 1) return '';
  return name.slice(dot + 1).toLowerCase();
}

export function FileTypeIcon({ name, className }: { name: string; className?: string }) {
  const kind = KINDS[extOf(name)] ?? DEFAULT_KIND;
  const Icon = kind.icon;
  return <Icon className={cn(kind.tint, className)} />;
}
