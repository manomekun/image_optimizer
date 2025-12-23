export interface ImageInfo {
  name: string;
  width: number;
  height: number;
  size: number;
  original_path: string;
}

export interface ResizeOptions {
  width: number | null;
  height: number | null;
  maintain_aspect_ratio: boolean;
}

export interface QuantOptions {
  quality: number;
}

export type OutputFormat = "png" | "webp";

export interface ProcessOptions {
  // リサイズ設定
  resize_enabled: boolean;
  width: number | null;
  height: number | null;
  maintain_aspect_ratio: boolean;
  // pngquant 設定 (PNG のみ)
  quantize_enabled: boolean;
  quality: number;
  // oxipng 最適化設定 (PNG のみ)
  optimize_enabled: boolean;
  // 出力先ディレクトリ (null の場合は元ファイルと同じ場所)
  output_dir: string | null;
  // 出力フォーマット
  output_format: OutputFormat;
}

export interface ProcessResult {
  success: boolean;
  original_size: number;
  result_size: number;
  output_path: string;
  message: string;
}

export interface ProgressPayload {
  completed: number;
  total: number;
  current_file: string | null;
  result: ProcessResult | null;
}
