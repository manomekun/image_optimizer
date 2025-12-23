<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { onMount, onDestroy, tick } from "svelte";
  import type { ImageInfo, ProcessOptions, ProcessResult, ProgressPayload, OutputFormat } from "$lib/types";

  // 状態管理
  let imageInfos = $state<ImageInfo[]>([]);
  let selectedImages = $state<string[]>([]);
  let isLoading = $state(false);
  let results = $state<ProcessResult[]>([]);

  // ドラッグ＆ドロップ状態
  let isDragging = $state(false);

  // 進捗状態
  let progress = $state({
    completed: 0,
    total: 0,
    currentFile: null as string | null,
    isProcessing: false,
  });

  // 処理オプション
  let resizeEnabled = $state(false);
  let resizeWidth = $state<number | null>(null);
  let resizeHeight = $state<number | null>(null);
  let maintainAspect = $state(true);

  let quantizeEnabled = $state(true);
  let quality = $state(80);

  let optimizeEnabled = $state(true);

  // 出力フォーマット
  let outputFormat = $state<OutputFormat>("png");

  // 出力先設定
  let outputDir = $state<string | null>(null);

  // イベントリスナー
  let unlistenProgress: UnlistenFn | null = null;
  let unlistenComplete: UnlistenFn | null = null;
  let unlistenDragDrop: UnlistenFn | null = null;

  // 画像ファイルかどうかを判定
  function isImageFile(path: string): boolean {
    const ext = path.toLowerCase().split('.').pop() || '';
    return ['png', 'jpg', 'jpeg', 'webp', 'gif'].includes(ext);
  }

  // ファイルパスから画像を読み込む
  async function loadImages(paths: string[]) {
    // 画像ファイルのみをフィルタリング
    const imagePaths = paths.filter(isImageFile);

    if (imagePaths.length === 0) {
      return;
    }

    isLoading = true;
    results = [];
    try {
      imageInfos = await invoke<ImageInfo[]>("get_image_info", {
        paths: imagePaths,
      });
      selectedImages = imagePaths;
    } catch (e) {
      console.error(e);
    } finally {
      isLoading = false;
    }
  }

  onMount(async () => {
    // 進捗イベントのリスナー登録
    unlistenProgress = await listen<ProgressPayload>("process-progress", (event) => {
      progress.completed = event.payload.completed;
      progress.total = event.payload.total;
      progress.currentFile = event.payload.current_file;

      // リアルタイムで結果を追加
      if (event.payload.result) {
        results = [...results, event.payload.result];
      }
    });

    // 処理完了イベントのリスナー登録
    unlistenComplete = await listen("process-complete", () => {
      isLoading = false;
      progress.isProcessing = false;
    });

    // ドラッグ＆ドロップイベントのリスナー登録
    unlistenDragDrop = await getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === 'over') {
        isDragging = true;
      } else if (event.payload.type === 'drop') {
        isDragging = false;
        loadImages(event.payload.paths);
      } else {
        // cancelled
        isDragging = false;
      }
    });
  });

  onDestroy(() => {
    unlistenProgress?.();
    unlistenComplete?.();
    unlistenDragDrop?.();
  });

  // ファイル選択
  async function selectFiles() {
    const files = await open({
      multiple: true,
      filters: [
        { name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif"] },
      ],
    });

    if (files && files.length > 0) {
      await loadImages(files as string[]);
    }
  }

  // 一括処理実行
  async function processImages() {
    if (selectedImages.length === 0) return;

    // PNG の場合: 少なくとも1つの処理が有効かチェック
    // WebP の場合: 常に変換処理が行われるのでチェック不要
    if (outputFormat === "png" && !resizeEnabled && !quantizeEnabled && !optimizeEnabled) {
      results = [
        {
          success: false,
          original_size: 0,
          result_size: 0,
          output_path: "",
          message: "少なくとも1つの処理を有効にしてください",
        },
      ];
      return;
    }

    // 進捗状態をリセット
    progress = {
      completed: 0,
      total: selectedImages.length,
      currentFile: null,
      isProcessing: true,
    };
    results = [];
    isLoading = true;

    // UI更新を確実に反映させるために tick() で待機
    await tick();

    const options: ProcessOptions = {
      resize_enabled: resizeEnabled,
      width: resizeWidth,
      height: resizeHeight,
      maintain_aspect_ratio: maintainAspect,
      quantize_enabled: quantizeEnabled,
      quality,
      optimize_enabled: optimizeEnabled,
      output_dir: outputDir,
      output_format: outputFormat,
    };

    try {
      // invoke はすぐに返る（処理は別スレッドで実行）
      // 完了は process-complete イベントで通知される
      await invoke<ProcessResult[]>("process_images", {
        paths: selectedImages,
        options,
      });
    } catch (e) {
      results = [
        {
          success: false,
          original_size: 0,
          result_size: 0,
          output_path: "",
          message: String(e),
        },
      ];
      isLoading = false;
      progress.isProcessing = false;
    }
  }

  // ファイルサイズをフォーマット
  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
  }

  // ファイル名のみを取得
  function getFileName(path: string): string {
    return path.split(/[/\\]/).pop() || path;
  }

  // 選択をクリア
  function clearSelection() {
    imageInfos = [];
    selectedImages = [];
    results = [];
  }

  // 出力フォルダ選択
  async function selectOutputDir() {
    const selected = await open({
      directory: true,
      multiple: false,
    });

    if (selected && typeof selected === "string") {
      outputDir = selected;
    }
  }

  // 出力フォルダをクリア
  function clearOutputDir() {
    outputDir = null;
  }

  // 進捗率を計算
  function getProgressPercent(): number {
    if (progress.total === 0) return 0;
    return (progress.completed / progress.total) * 100;
  }
</script>

<main class="container">
  <h1>Image Optimizer</h1>

  <!-- ドラッグ＆ドロップオーバーレイ -->
  {#if isDragging}
    <div class="drop-overlay">
      <div class="drop-content">
        <span class="drop-icon">+</span>
        <p>ここに画像をドロップ</p>
      </div>
    </div>
  {/if}

  <!-- ドロップゾーン（画像未選択時のみ表示） -->
  {#if imageInfos.length === 0}
    <div class="drop-zone" class:active={isDragging}>
      <div class="drop-zone-content">
        <span class="drop-zone-icon">+</span>
        <p class="drop-zone-text">画像をドラッグ＆ドロップ</p>
        <p class="drop-zone-hint">または</p>
        <button onclick={selectFiles} disabled={isLoading}>
          {isLoading ? "読み込み中..." : "ファイルを選択"}
        </button>
        <p class="drop-zone-formats">PNG, JPG, JPEG, WebP, GIF</p>
      </div>
    </div>
  {:else}
    <!-- ファイル選択ボタン（画像選択後） -->
    <div class="actions">
      <button onclick={selectFiles} disabled={isLoading}>
        {isLoading ? "読み込み中..." : "画像を追加"}
      </button>
      <button class="secondary" onclick={clearSelection}>クリア</button>
    </div>
  {/if}

  <!-- 画像リスト表示 -->
  {#if imageInfos.length > 0}
    <section class="image-list">
      <h2>選択された画像 ({imageInfos.length}枚)</h2>
      <div class="file-list">
        {#each imageInfos as info}
          <div class="file-item">
            <span class="file-name" title={info.name}>{info.name}</span>
            <span class="file-meta">{info.width} x {info.height}</span>
            <span class="file-size">{formatSize(info.size)}</span>
          </div>
        {/each}
      </div>
    </section>

    <!-- 処理オプション -->
    <section class="options-panel">
      <h2>処理オプション</h2>

      <!-- 出力フォーマット選択 -->
      <div class="option-group format-group">
        <div class="option-header">
          <span class="format-label">出力フォーマット</span>
        </div>
        <div class="option-content format-content">
          <div class="format-buttons">
            <button
              type="button"
              class="format-btn"
              class:active={outputFormat === "png"}
              onclick={() => outputFormat = "png"}
            >
              PNG
            </button>
            <button
              type="button"
              class="format-btn"
              class:active={outputFormat === "webp"}
              onclick={() => outputFormat = "webp"}
            >
              WebP
            </button>
          </div>
          <p class="hint">
            {#if outputFormat === "png"}
              PNG: 可逆圧縮、透過対応、pngquant/oxipng で最適化
            {:else}
              WebP: 高圧縮率、透過対応、モダンブラウザ対応
            {/if}
          </p>
        </div>
      </div>

      <p class="pipeline-info">
        {#if outputFormat === "png"}
          処理順序: リサイズ → pngquant圧縮 → PNG最適化
        {:else}
          処理順序: リサイズ → WebP変換
        {/if}
      </p>

      <!-- リサイズ -->
      <div class="option-group">
        <label class="option-header">
          <input type="checkbox" bind:checked={resizeEnabled} />
          リサイズ
        </label>
        {#if resizeEnabled}
          <div class="option-content">
            <div class="input-row">
              <label>
                幅 (px)
                <input
                  type="number"
                  bind:value={resizeWidth}
                  min="1"
                  placeholder="指定なし"
                />
              </label>
              <label>
                高さ (px)
                <input
                  type="number"
                  bind:value={resizeHeight}
                  min="1"
                  placeholder="指定なし"
                />
              </label>
            </div>
            <label class="checkbox">
              <input type="checkbox" bind:checked={maintainAspect} />
              アスペクト比を維持
            </label>
          </div>
        {/if}
      </div>

      <!-- クオリティ設定 (共通) -->
      <div class="option-group">
        <div class="option-header">
          <span>クオリティ: {quality}</span>
        </div>
        <div class="option-content">
          <label class="slider-label">
            <input type="range" bind:value={quality} min="1" max="100" />
          </label>
          <p class="hint">
            {#if outputFormat === "png"}
              pngquant の品質設定（値が高いほど高品質）
            {:else}
              WebP の品質設定（100でロスレス圧縮）
            {/if}
          </p>
        </div>
      </div>

      <!-- PNG専用オプション -->
      {#if outputFormat === "png"}
        <!-- pngquant 圧縮 -->
        <div class="option-group">
          <label class="option-header">
            <input type="checkbox" bind:checked={quantizeEnabled} />
            pngquant 圧縮（非可逆）
          </label>
          {#if quantizeEnabled}
            <div class="option-content">
              <p class="description">色数を削減して圧縮します</p>
            </div>
          {/if}
        </div>

        <!-- oxipng 最適化 -->
        <div class="option-group">
          <label class="option-header">
            <input type="checkbox" bind:checked={optimizeEnabled} />
            PNG 最適化（可逆）
          </label>
          {#if optimizeEnabled}
            <div class="option-content">
              <p class="description">oxipng によるロスレス圧縮を実行します</p>
            </div>
          {/if}
        </div>
      {/if}

      <!-- 出力先フォルダ設定 -->
      <div class="option-group output-group">
        <div class="option-header output-header">
          <span class="output-label">📁 出力先フォルダ</span>
        </div>
        <div class="option-content output-content">
          <div class="output-row">
            <input
              type="text"
              class="output-path-input"
              value={outputDir || ""}
              placeholder="元ファイルと同じ場所"
              readonly
            />
            <button type="button" class="output-btn" onclick={selectOutputDir}>
              選択
            </button>
            {#if outputDir}
              <button type="button" class="output-btn clear-btn" onclick={clearOutputDir}>
                クリア
              </button>
            {/if}
          </div>
          <p class="hint">未指定の場合、元ファイルと同じ場所に出力されます</p>
        </div>
      </div>

      <!-- 実行ボタン -->
      <button class="primary execute-btn" onclick={processImages} disabled={isLoading}>
        {isLoading ? "処理中..." : "一括処理を実行"}
      </button>
    </section>
  {/if}

  <!-- プログレスバー -->
  {#if progress.isProcessing}
    <section class="progress-section">
      <div class="progress-container">
        <!-- スピナー -->
        <div class="spinner"></div>

        <!-- プログレスバー -->
        <div class="progress-bar-wrapper">
          <div
            class="progress-bar"
            style="width: {getProgressPercent()}%"
          ></div>
        </div>

        <!-- 進捗テキスト -->
        <p class="progress-text">
          {progress.completed} / {progress.total} 枚処理完了
        </p>

        <!-- 現在のファイル名 -->
        {#if progress.currentFile}
          <p class="current-file">{getFileName(progress.currentFile)}</p>
        {/if}
      </div>
    </section>
  {/if}

  <!-- 結果表示 -->
  {#if results.length > 0}
    <section class="results">
      <h2>処理結果</h2>
      {#each results as result}
        <div
          class="result-item"
          class:success={result.success}
          class:error={!result.success}
        >
          <p class="result-message">{result.message}</p>
          {#if result.success && result.output_path}
            <p class="output-path">出力: {result.output_path}</p>
          {/if}
        </div>
      {/each}
    </section>
  {/if}
</main>

<style>
  :root {
    font-family: Inter, Avenir, Helvetica, Arial, sans-serif;
    font-size: 16px;
    line-height: 1.5;
    font-weight: 400;
    color: #0f0f0f;
    background-color: #f6f6f6;
    font-synthesis: none;
    text-rendering: optimizeLegibility;
    -webkit-font-smoothing: antialiased;
    -moz-osx-font-smoothing: grayscale;
  }

  .container {
    max-width: 900px;
    margin: 0 auto;
    padding: 2rem;
    position: relative;
  }

  /* ドラッグ＆ドロップオーバーレイ */
  .drop-overlay {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: rgba(57, 108, 216, 0.9);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    pointer-events: none;
  }

  .drop-content {
    text-align: center;
    color: white;
  }

  .drop-icon {
    font-size: 6rem;
    display: block;
    margin-bottom: 1rem;
    font-weight: 300;
  }

  .drop-content p {
    font-size: 1.5rem;
    font-weight: 500;
    margin: 0;
  }

  /* ドロップゾーン */
  .drop-zone {
    border: 3px dashed #ccc;
    border-radius: 12px;
    padding: 3rem 2rem;
    text-align: center;
    transition: all 0.2s;
    margin-bottom: 2rem;
    background: white;
  }

  .drop-zone:hover,
  .drop-zone.active {
    border-color: #396cd8;
    background: #f0f5ff;
  }

  .drop-zone-content {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.5rem;
  }

  .drop-zone-icon {
    font-size: 3rem;
    color: #999;
    font-weight: 300;
    line-height: 1;
  }

  .drop-zone:hover .drop-zone-icon,
  .drop-zone.active .drop-zone-icon {
    color: #396cd8;
  }

  .drop-zone-text {
    font-size: 1.25rem;
    color: #444;
    margin: 0;
  }

  .drop-zone-hint {
    color: #888;
    margin: 0.5rem 0;
    font-size: 0.875rem;
  }

  .drop-zone-formats {
    color: #aaa;
    font-size: 0.75rem;
    margin-top: 0.5rem;
  }

  h1 {
    text-align: center;
    margin-bottom: 2rem;
    color: #333;
  }

  h2 {
    font-size: 1.25rem;
    margin-bottom: 1rem;
    color: #444;
  }

  .actions {
    display: flex;
    gap: 0.5rem;
    justify-content: center;
    margin-bottom: 2rem;
  }

  button {
    border-radius: 8px;
    border: 1px solid transparent;
    padding: 0.6em 1.2em;
    font-size: 1em;
    font-weight: 500;
    font-family: inherit;
    background-color: #ffffff;
    color: #0f0f0f;
    cursor: pointer;
    transition: all 0.2s;
    box-shadow: 0 2px 4px rgba(0, 0, 0, 0.1);
  }

  button:hover {
    border-color: #396cd8;
  }

  button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  button.primary {
    background-color: #396cd8;
    color: white;
  }

  button.primary:hover {
    background-color: #2d5bb8;
  }

  button.secondary {
    background-color: #666;
    color: white;
  }

  button.secondary:hover {
    background-color: #555;
  }

  .image-list {
    margin-bottom: 2rem;
  }

  .file-list {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    max-height: 200px;
    overflow-y: auto;
    background: white;
    border: 1px solid #ddd;
    border-radius: 8px;
    padding: 0.5rem;
  }

  .file-item {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 0.5rem 0.75rem;
    border-radius: 4px;
    font-size: 0.875rem;
  }

  .file-item:hover {
    background: #f5f5f5;
  }

  .file-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: #333;
  }

  .file-meta {
    color: #888;
    font-size: 0.75rem;
    white-space: nowrap;
  }

  .file-size {
    color: #666;
    font-size: 0.75rem;
    white-space: nowrap;
    min-width: 70px;
    text-align: right;
  }

  /* オプションパネル */
  .options-panel {
    background: white;
    border: 1px solid #ddd;
    border-radius: 8px;
    padding: 1.5rem;
    margin-bottom: 2rem;
  }

  /* フォーマット選択 */
  .format-group {
    background: #f0f5ff;
    border-color: #396cd8;
  }

  .format-label {
    font-weight: 600;
  }

  .format-content {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .format-buttons {
    display: flex;
    gap: 0.5rem;
  }

  .format-btn {
    flex: 1;
    padding: 0.75rem 1rem;
    border: 2px solid #ccc;
    border-radius: 8px;
    background: white;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.2s;
  }

  .format-btn:hover {
    border-color: #396cd8;
  }

  .format-btn.active {
    border-color: #396cd8;
    background: #396cd8;
    color: white;
  }

  .pipeline-info {
    color: #666;
    font-size: 0.875rem;
    margin-bottom: 1.5rem;
    padding: 0.5rem;
    background: #f5f5f5;
    border-radius: 4px;
    text-align: center;
  }

  .option-group {
    border: 1px solid #eee;
    border-radius: 8px;
    margin-bottom: 1rem;
    overflow: hidden;
  }

  .option-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 1rem;
    background: #fafafa;
    cursor: pointer;
    font-weight: 500;
    margin: 0;
  }

  .option-header input[type="checkbox"] {
    width: 1.25rem;
    height: 1.25rem;
    cursor: pointer;
  }

  .step-number {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.5rem;
    height: 1.5rem;
    background: #396cd8;
    color: white;
    border-radius: 50%;
    font-size: 0.75rem;
    font-weight: 600;
  }

  .option-content {
    padding: 1rem;
    border-top: 1px solid #eee;
  }

  .input-row {
    display: flex;
    gap: 1rem;
    margin-bottom: 0.75rem;
  }

  .input-row label {
    flex: 1;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.875rem;
    color: #444;
  }

  input[type="number"] {
    padding: 0.5rem;
    border: 1px solid #ccc;
    border-radius: 4px;
    font-size: 1rem;
  }

  input[type="number"]:focus {
    outline: none;
    border-color: #396cd8;
  }

  .checkbox {
    flex-direction: row;
    align-items: center;
    gap: 0.5rem;
    cursor: pointer;
  }

  .checkbox input {
    width: 1rem;
    height: 1rem;
    cursor: pointer;
  }

  .slider-label {
    gap: 0.5rem;
  }

  input[type="range"] {
    width: 100%;
    cursor: pointer;
  }

  .hint {
    font-size: 0.75rem;
    color: #888;
    margin: 0.5rem 0 0;
  }

  .description {
    color: #666;
    margin: 0;
    font-size: 0.875rem;
  }

  .execute-btn {
    width: 100%;
    padding: 1rem;
    font-size: 1.1rem;
    margin-top: 1rem;
  }

  /* 出力先フォルダ設定 */
  .output-group {
    background: #f8f9fa;
  }

  .output-header {
    cursor: default;
  }

  .output-label {
    font-weight: 500;
  }

  .output-content {
    padding: 1rem;
  }

  .output-row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }

  .output-path-input {
    flex: 1;
    padding: 0.5rem 0.75rem;
    border: 1px solid #ccc;
    border-radius: 4px;
    font-size: 0.875rem;
    background: white;
    color: #333;
  }

  .output-path-input::placeholder {
    color: #999;
  }

  .output-btn {
    padding: 0.5rem 1rem;
    font-size: 0.875rem;
    white-space: nowrap;
  }

  .clear-btn {
    background-color: #dc3545;
    color: white;
  }

  .clear-btn:hover {
    background-color: #c82333;
  }

  /* プログレスセクション */
  .progress-section {
    margin: 1.5rem 0;
    padding: 1.5rem;
    background: white;
    border: 1px solid #ddd;
    border-radius: 8px;
  }

  .progress-container {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1rem;
  }

  .spinner {
    width: 40px;
    height: 40px;
    border: 4px solid #e0e0e0;
    border-top-color: #396cd8;
    border-radius: 50%;
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .progress-bar-wrapper {
    width: 100%;
    height: 8px;
    background: #e0e0e0;
    border-radius: 4px;
    overflow: hidden;
  }

  .progress-bar {
    height: 100%;
    background: linear-gradient(90deg, #396cd8, #5a8dee);
    border-radius: 4px;
    transition: width 0.3s ease;
  }

  .progress-text {
    font-weight: 600;
    color: #333;
    margin: 0;
  }

  .current-file {
    font-size: 0.875rem;
    color: #666;
    margin: 0;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .results {
    margin-top: 2rem;
  }

  .result-item {
    padding: 1rem;
    border-radius: 8px;
    margin-bottom: 0.5rem;
  }

  .result-item.success {
    background: #d4edda;
    border: 1px solid #c3e6cb;
    color: #155724;
  }

  .result-item.error {
    background: #f8d7da;
    border: 1px solid #f5c6cb;
    color: #721c24;
  }

  .result-message {
    margin: 0;
    font-weight: 500;
  }

  .output-path {
    font-size: 0.75rem;
    opacity: 0.8;
    margin: 0.5rem 0 0;
    word-break: break-all;
  }

  @media (prefers-color-scheme: dark) {
    :root {
      color: #f6f6f6;
      background-color: #1a1a1a;
    }

    h1 {
      color: #f6f6f6;
    }

    h2 {
      color: #ddd;
    }

    button {
      color: #f6f6f6;
      background-color: #2a2a2a;
    }

    .file-list {
      background: #2a2a2a;
      border-color: #444;
    }

    .file-item:hover {
      background: #333;
    }

    .file-name {
      color: #f6f6f6;
    }

    .file-meta {
      color: #888;
    }

    .file-size {
      color: #aaa;
    }

    .options-panel {
      background: #2a2a2a;
      border-color: #444;
    }

    .pipeline-info {
      background: #333;
      color: #aaa;
    }

    .option-group {
      border-color: #444;
    }

    .option-header {
      background: #333;
      color: #f6f6f6;
    }

    .option-content {
      border-top-color: #444;
    }

    label {
      color: #ccc;
    }

    input[type="number"] {
      background: #2a2a2a;
      border-color: #555;
      color: #f6f6f6;
    }

    input[type="number"]:focus {
      border-color: #6ea8fe;
    }

    .hint {
      color: #888;
    }

    .description {
      color: #aaa;
    }

    /* プログレスセクション (ダークモード) */
    .progress-section {
      background: #2a2a2a;
      border-color: #444;
    }

    .spinner {
      border-color: #444;
      border-top-color: #6ea8fe;
    }

    .progress-bar-wrapper {
      background: #444;
    }

    .progress-bar {
      background: linear-gradient(90deg, #6ea8fe, #8fc1ff);
    }

    .progress-text {
      color: #f6f6f6;
    }

    .current-file {
      color: #aaa;
    }

    .result-item.success {
      background: #1e4620;
      border-color: #2d6a30;
      color: #a3d9a5;
    }

    .result-item.error {
      background: #4a1a1a;
      border-color: #6a2d2d;
      color: #f5a5a5;
    }

    /* 出力先フォルダ設定 (ダークモード) */
    .output-group {
      background: #333;
    }

    .output-path-input {
      background: #2a2a2a;
      border-color: #555;
      color: #f6f6f6;
    }

    .output-path-input::placeholder {
      color: #888;
    }

    /* フォーマット選択 (ダークモード) */
    .format-group {
      background: #2a3a5a;
      border-color: #6ea8fe;
    }

    .format-btn {
      background: #2a2a2a;
      border-color: #555;
      color: #f6f6f6;
    }

    .format-btn:hover {
      border-color: #6ea8fe;
    }

    .format-btn.active {
      border-color: #6ea8fe;
      background: #6ea8fe;
      color: #1a1a1a;
    }

    /* ドロップゾーン (ダークモード) */
    .drop-zone {
      background: #2a2a2a;
      border-color: #555;
    }

    .drop-zone:hover,
    .drop-zone.active {
      border-color: #6ea8fe;
      background: #2a3a5a;
    }

    .drop-zone-icon {
      color: #888;
    }

    .drop-zone:hover .drop-zone-icon,
    .drop-zone.active .drop-zone-icon {
      color: #6ea8fe;
    }

    .drop-zone-text {
      color: #ddd;
    }

    .drop-zone-hint {
      color: #888;
    }

    .drop-zone-formats {
      color: #666;
    }

    .drop-overlay {
      background: rgba(110, 168, 254, 0.9);
    }

    .clear-btn {
      background-color: #c82333;
    }

    .clear-btn:hover {
      background-color: #a71d2a;
    }
  }
</style>
