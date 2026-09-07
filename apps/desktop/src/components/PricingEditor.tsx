import { useEffect, useRef, useState } from "react";
import {
  ModelPriceView,
  ModelPriceSuggestionView,
  PriceTableView,
  PricingInventoryView,
  getPricingInventory,
  syncModelPrices,
  setPriceSyncEnabled,
  removeModelPrice,
  setModelPrice,
  suggestModelPrice,
} from "../api";
import { useLocalizedCopy } from "./LanguageProvider";
import { humanizeAppError } from "../errors";
import { useDraftGuard, useDraftNavigation } from "./DraftNavigation";
import { useErrorToast } from "./ErrorToast";
import { Button } from "./ui/button";
import { Switch } from "./ui/switch";
import { Field, FieldGroup, FieldLabel } from "./ui/field";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "./ui/table";

function displayRate(rate: number | null | undefined): string {
  return rate == null ? "" : String(rate / 1_000_000);
}

function rateMicros(raw: string, invalidMessage: string): number {
  const value = Number(raw);
  const micros = Math.round(value * 1_000_000);
  if (!/^\d+(?:\.\d{1,6})?$/.test(raw)
      || !Number.isFinite(value)
      || value < 0
      || value > 9_000_000_000
      || !Number.isSafeInteger(micros)) {
    throw new Error(invalidMessage);
  }
  return micros;
}

export default function PricingEditor() {
  const { copy } = useLocalizedCopy();
  const { dismissToast, showError, showInfo, showSuccess } = useErrorToast();
  const [table, setTable] = useState<PriceTableView | null>(null);
  const [inventory, setInventory] = useState<PricingInventoryView | null>(null);
  const [model, setModel] = useState("");
  const modelInput = useRef<HTMLInputElement>(null);
  const [input, setInput] = useState("0");
  const [output, setOutput] = useState("0");
  const [cacheRead, setCacheRead] = useState("0");
  const [cacheWrite, setCacheWrite] = useState("0");
  const [cacheWrite5m, setCacheWrite5m] = useState("");
  const [cacheWrite1h, setCacheWrite1h] = useState("");
  const [reasoning, setReasoning] = useState("");
  const [error, setError] = useState("");
  const [observationError, setObservationError] = useState("");
  const [dirty, setDirty] = useState(false);
  const [working, setBusy] = useState(false);
  const busy = working || inventory?.sync.running === true;
  const inFlight = useRef(false);
  const draftEpoch = useRef(0);
  const markDirty = () => {
    draftEpoch.current += 1;
    setDirty(true);
  };
  const confirmNavigation = useDraftNavigation();
  useDraftGuard(dirty);
  const receiveInventory = (value: PricingInventoryView) => {
    setInventory(value);
    setTable(value.table);
    setObservationError("");
  };
  const reloadInventory = async () => {
    const current = await getPricingInventory();
    receiveInventory(current);
    return current.table;
  };
  const reloadAfterError = async (value: unknown) => {
    showError(humanizeAppError(value), "model-price-write");
    try {
      const current = await reloadInventory();
      if (table && current.version !== table.version) setError(copy(
        `Prices changed to v${current.version}. Your draft is preserved. Review the current table before saving again.`,
        `价格表已更新为 v${current.version}，已保留输入。请核对当前价格表后再次保存。`,
        `價格表已更新為 v${current.version}，已保留輸入。請核對目前價格表後再次儲存。`,
        `価格表は v${current.version} に更新されました。入力は保持されています。現在の価格表を確認してから再保存してください。`,
      ));
    } catch {
      setError(copy("Could not reload prices. Retry the refresh before saving.", "无法刷新价格表，请刷新后再保存。", "無法重新整理價格表，請重新整理後再儲存。", "価格表を再取得できません。更新してから保存してください。"));
    }
  };
  // A dirty price belongs to one model only. Reusing it after the model ID
  // changes can silently save one model's price under another model.
  const [touchedModel, setTouchedModel] = useState<string | null>(null);
  const [lookupRequestedFor, setLookupRequestedFor] = useState<string | null>(null);
  const [suggestion, setSuggestion] = useState<ModelPriceSuggestionView | null>(null);
  const noPublicPriceMessage = copy(
    "No public price was found.",
    "未找到公开价格。", "未找到公開價格。", "公開価格が見つかりません。"
  );

  useEffect(() => {
    let cancelled = false;
    getPricingInventory().then((value) => {
      if (!cancelled) receiveInventory(value);
    }).catch((value) => { if (!cancelled) setError(humanizeAppError(value)); });
    return () => { cancelled = true; };
  }, []);

  // The scheduler can start after opt-in returns. Observe idle enabled state too.
  // Reject late observations when an edit or foreground operation has started.
  useEffect(() => {
    if (!inventory || (!inventory.sync.enabled && !inventory.sync.running) || dirty || working) return;
    let cancelled = false;
    const epoch = draftEpoch.current;
    const delay = inventory.sync.running ? 1500 : 15000;
    const isCurrent = () => !cancelled && !inFlight.current && draftEpoch.current === epoch;
    let timer: number;
    const poll = async () => {
      try {
        if (isCurrent()) {
          const value = await getPricingInventory();
          if (isCurrent()) receiveInventory(value);
        }
      } catch (value) {
        if (isCurrent()) setObservationError(humanizeAppError(value));
      } finally {
        // Retry failures without overlapping requests. A fresh inventory resets the delay.
        if (!cancelled) timer = window.setTimeout(() => void poll(), delay);
      }
    };
    timer = window.setTimeout(() => void poll(), delay);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [inventory, dirty, working]);

  const cancelEdit = async () => {
    if (inFlight.current || working) return;
    inFlight.current = true;
    draftEpoch.current += 1;
    setBusy(true);
    try {
      await reloadInventory();
      setModel("");
      setInput("0"); setOutput("0"); setCacheRead("0"); setCacheWrite("0");
      setCacheWrite5m(""); setCacheWrite1h(""); setReasoning("");
      setTouchedModel(null); setLookupRequestedFor(null); setSuggestion(null);
      setDirty(false);
      setError("");
      modelInput.current?.focus();
    } catch (value) {
      showError(humanizeAppError(value), "model-price-cancel");
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  const updateSync = async (enabled?: boolean) => {
    if (!inventory || dirty || busy || inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError("");
    try {
      receiveInventory(await (enabled === undefined ? syncModelPrices() : setPriceSyncEnabled(enabled)));
    } catch (value) {
      showError(humanizeAppError(value), "model-price-sync");
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  useEffect(() => {
    const requestedModel = model.trim();
    setSuggestion(null);
    if (!table
        || requestedModel.length === 0
        || requestedModel.length > 256
        || table.models[requestedModel]
        || lookupRequestedFor !== requestedModel
        || touchedModel === requestedModel) {
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      suggestModelPrice(null, requestedModel)
        .then((value) => {
          if (cancelled) return;
          if (!value) {
            setLookupRequestedFor(null);
            showInfo(noPublicPriceMessage, `model-price-suggest:${requestedModel}`);
            return;
          }
          setInput(displayRate(value.input_per_mtok));
          setOutput(displayRate(value.output_per_mtok));
          setCacheRead(displayRate(value.cache_read_per_mtok));
          setCacheWrite(displayRate(value.cache_write_per_mtok));
          setReasoning(displayRate(value.reasoning_per_mtok));
          setCacheWrite5m(""); setCacheWrite1h("");
          markDirty();
          setSuggestion(value);
          dismissToast(`model-price-suggest:${requestedModel}`);
        })
        // Suggestions are optional. Manual entry remains available offline,
        // but an explicit lookup must not fail silently.
        .catch((value) => {
          if (!cancelled) {
            setLookupRequestedFor(null);
            showError(humanizeAppError(value), `model-price-suggest:${requestedModel}`);
          }
        });
    }, 350);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [dismissToast, lookupRequestedFor, model, noPublicPriceMessage, showError, showInfo, table, touchedModel]);

  const edit = (name: string, price: ModelPriceView | null) => {
    markDirty();
    setModel(name);
    setInput(displayRate(price?.input_per_mtok));
    setOutput(displayRate(price?.output_per_mtok));
    setCacheRead(displayRate(price?.cache_read_per_mtok));
    setCacheWrite(displayRate(price?.cache_write_per_mtok));
    setCacheWrite5m(displayRate(price?.cache_write_5m_per_mtok));
    setCacheWrite1h(displayRate(price?.cache_write_1h_per_mtok));
    setReasoning(displayRate(price?.reasoning_per_mtok));
    setError("");
    setTouchedModel(name);
    setSuggestion(null);
    setLookupRequestedFor(null);
    modelInput.current?.focus();
  };

  const save = async () => {
    setError("");
    if (!table || busy || inFlight.current) return;
    if (!model || model.trim() !== model || model.length > 256) {
      setError(copy(
        "Model ID must be 1–256 characters with no leading or trailing spaces.",
        "模型 ID 必须是 1–256 个字符，且首尾不能有空格。", "模型 ID 必須是 1–256 個字元，且首尾不能有空格。", "モデル ID は 1～256 文字で、先頭と末尾にスペースを含めないでください。"
      ));
      return;
    }
    let price: ModelPriceView;
    try {
      const optionalRate = (raw: string) => raw === "" ? null : rateMicros(raw, copy(
        "Cache TTL price must be a valid amount with at most 6 decimal places.",
        "缓存 TTL 价格必须是最多 6 位小数的有效金额。", "快取 TTL 價格必須是最多 6 位小數的有效金額。", "キャッシュ TTL 価格は小数点以下最大 6 桁の有効な金額にしてください。"));
      price = {
        ...(cacheWrite5m !== "" || table.models[model]?.cache_write_5m_per_mtok != null ? { cache_write_5m_per_mtok: optionalRate(cacheWrite5m) } : {}),
        ...(cacheWrite1h !== "" || table.models[model]?.cache_write_1h_per_mtok != null ? { cache_write_1h_per_mtok: optionalRate(cacheWrite1h) } : {}),
        input_per_mtok: rateMicros(input, copy(
          "Input price must be a valid amount from 0 to 9 billion with at most 6 decimal places.",
          "输入价格必须是 0 到 90 亿之间、最多 6 位小数的有效金额。", "輸入價格必須介於 0 到 90 億之間，小數點後最多 6 位。", "入力価格は 0～90億の有効な金額で、小数点以下は最大6桁です。"
        )),
        output_per_mtok: rateMicros(output, copy(
          "Output price must be a valid amount from 0 to 9 billion with at most 6 decimal places.",
          "输出价格必须是 0 到 90 亿之间、最多 6 位小数的有效金额。", "輸出價格必須介於 0 到 90 億之間，小數點後最多 6 位。", "出力価格は 0～90億の有効な金額で、小数点以下は最大6桁です。"
        )),
        cache_read_per_mtok: rateMicros(cacheRead, copy(
          "Cache read price must be a valid amount from 0 to 9 billion with at most 6 decimal places.",
          "缓存读取价格必须是 0 到 90 亿之间、最多 6 位小数的有效金额。", "快取讀取價格必須介於 0 到 90 億之間，小數點後最多 6 位。", "キャッシュ読み取り価格は 0～90億の有効な金額で、小数点以下は最大6桁です。"
        )),
        cache_write_per_mtok: rateMicros(cacheWrite, copy(
          "Cache write price must be a valid amount from 0 to 9 billion with at most 6 decimal places.",
          "缓存写入价格必须是 0 到 90 亿之间、最多 6 位小数的有效金额。", "快取寫入價格必須介於 0 到 90 億之間，小數點後最多 6 位。", "キャッシュ書き込み価格は 0～90億の有効な金額で、小数点以下は最大6桁です。"
        )),
        reasoning_per_mtok: reasoning === "" ? null : rateMicros(reasoning, copy(
          "Reasoning price must be a valid amount from 0 to 9 billion with at most 6 decimal places.",
          "推理价格必须是 0 到 90 亿之间、最多 6 位小数的有效金额。", "推理價格必須介於 0 到 90 億之間，小數點後最多 6 位。", "推論価格は 0～90億の有効な金額で、小数点以下は最大6桁です。"
        )),
      };
    } catch (value) {
      setError(humanizeAppError(value));
      return;
    }
    inFlight.current = true;
    setBusy(true);
    try {
      const next = await setModelPrice(model, price, table.version);
      setTable(next);
      setDirty(false);
      try { await reloadInventory(); }
      catch (value) { setError(humanizeAppError(value)); }
      showSuccess(copy(
        `Created price v${next.version}. Reapply the configuration to update the running proxy.`,
        `已生成 price v${next.version}；正在运行的代理需重新应用配置。`, `已生成 price v${next.version}；正在執行的代理需重新應用配置。`, `price v${next.version} が生成されました；実行中のプロキシは構成を再適用する必要があります。`
      ), `model-price-save:${model}`);
    } catch (value) {
      await reloadAfterError(value);
    } finally {
      inFlight.current = false; setBusy(false);
    }
  };

  const remove = async (name: string) => {
    if (!table || dirty || busy || inFlight.current) return;
    inFlight.current = true; setBusy(true);
    setError("");
    try {
      const next = await removeModelPrice(name, table.version);
      setTable(next);
      try { await reloadInventory(); }
      catch (value) { setError(humanizeAppError(value)); }
      if (model === name) {
        setDirty(false);
        setModel("");
        setInput("0");
        setOutput("0");
        setCacheRead("0");
        setCacheWrite("0");
        setCacheWrite5m(""); setCacheWrite1h("");
        setReasoning("");
        setTouchedModel(null);
        setLookupRequestedFor(null);
        setSuggestion(null);
      }
      showSuccess(copy(
        `Created price v${next.version}. Historical receipts keep their original cost.`,
        `已生成 price v${next.version}；历史回执保持原成本。`, `已生成 price v${next.version}；歷史回執保持原成本。`, `price v${next.version} が生成されました；履歴レシートは元のコストを保持します。`
      ), `model-price-remove:${name}`);
    } catch (value) {
      await reloadAfterError(value);
    } finally {
      inFlight.current = false; setBusy(false);
    }
  };

  const offerings = inventory?.offerings ?? [];
  const offeringKeys = new Set(offerings.map((offering) => offering.key));
  const legacyPrices = Object.entries(table?.models ?? {}).filter(([key]) => !offeringKeys.has(key));
  const configured = offerings.filter((offering) => offering.price != null && ["manual", "models.dev", "provider"].includes(offering.source)).length;
  const fallback = offerings.filter((offering) => offering.price != null && offering.source === "fallback").length;
  const pending = offerings.length - configured - fallback;
  const sourceLabel = (source: string) => {
    switch (source) {
      case "manual": return copy("Manual", "手动价格", "手動價格", "手動価格");
      case "models.dev": return copy("models.dev · estimate rate", "models.dev · 估算基准", "models.dev · 估算基準", "models.dev・見積単価");
      case "provider": return copy("Provider · estimate rate", "渠道公开价 · 估算基准", "渠道公開價 · 估算基準", "プロバイダー公開価格・見積単価");
      case "fallback": return copy("Generic reference · channel unverified", "通用参考价 · 渠道未核验", "通用參考價 · 渠道未核驗", "共通参考価格・経路未確認");
      default: return copy("Pending price", "待补价", "待補價", "価格未設定");
    }
  };
  const timeLabel = (value: number | null) => value == null ? "—" : new Date(value).toLocaleString();

  return (
    <div className="pricing-section">
      <div className="budget-title-row">
        <div>
          <h3>{copy("Versioned model pricing", "版本化模型定价", "版本化模型定價", "バージョン化モデル価格")}</h3>
          <p>{copy(
            "Base rates use USD / 1M tokens for estimates, not billed charges. Price changes create versions. Priced receipts keep their cost; sync can backfill eligible missing estimates.",
            "基准价格：USD / 1M tokens，用于估算，非账单扣费。改价生成新版本；已计价历史回执不会重算，同步可补齐符合条件的缺失估算。", "基準價格：USD / 1M tokens，用於估算，非帳單扣費。改價產生新版本；已計價歷史回執不會重新計算，同步可補齊符合條件的缺失估算。", "基準単価は USD / 1M tokens で、見積用です。請求額ではありません。価格変更はバージョン化されます。計算済みの履歴コストは保持し、同期は計算可能な未算出コストのみ補完します。"
          )}</p>
        </div>
        <span className="price-version">price v{table?.version ?? "—"}</span>
      </div>

      {inventory && <div className="flex min-w-0 flex-col gap-3" aria-busy={busy}>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="sub" role="status">{copy(
            `${configured} / ${offerings.length} channel prices · ${fallback} generic references · ${pending} pending`,
            `${configured} / ${offerings.length} 已配置渠道价格 · ${fallback} 通用参考价 · ${pending} 待补价`,
            `${configured} / ${offerings.length} 已設定渠道價格 · ${fallback} 通用參考價 · ${pending} 待補價`,
            `${configured} / ${offerings.length} 経路価格設定済み・${fallback} 共通参考価格・${pending} 未設定`,
          )}</p>
          <div className="flex flex-wrap items-center gap-4">
            <FieldGroup className="w-auto">
              <Field orientation="horizontal" data-disabled={dirty || busy}>
                <FieldLabel htmlFor="price-auto-sync">{copy("Automatic price sync", "自动同步价格", "自動同步價格", "価格の自動同期")}</FieldLabel>
                <Switch id="price-auto-sync" checked={inventory.sync.enabled} disabled={dirty || busy} onCheckedChange={(enabled) => void updateSync(enabled)} />
              </Field>
            </FieldGroup>
            <Button variant="outline" size="sm" disabled={dirty || busy} onClick={() => void updateSync()}>
              {busy ? copy("Synchronizing…", "同步中…", "同步中…", "同期中…") : copy("Sync now", "立即同步", "立即同步", "今すぐ同期")}
            </Button>
          </div>
        </div>
        <p className="sub">{copy("Last successful sync", "最近成功同步", "最近成功同步", "最終同期成功")}: {timeLabel(inventory.sync.last_sync_ms)}
          {inventory.sync.last_attempt_ms !== inventory.sync.last_sync_ms && <> · {copy("Last attempt", "最近尝试", "最近嘗試", "最終試行")}: {timeLabel(inventory.sync.last_attempt_ms)}</>}
          {" · "}{copy("Automatic sync preserves manual prices.", "自动同步保留手动价格。", "自動同步保留手動價格。", "自動同期は手動価格を保持します。")}
        </p>
        {dirty && <p className="sub">{copy("Save or cancel the current price draft before syncing.", "请先保存或取消当前价格草稿，再同步价格。", "請先儲存或取消目前價格草稿，再同步價格。", "同期前に現在の価格下書きを保存またはキャンセルしてください。")}</p>}
        {inventory.requires_apply && <p className="sub">{copy("Prices are saved. Apply the configuration to update the running proxy.", "价格已保存；运行中的代理仍需应用配置。", "價格已儲存；執行中的代理仍需套用設定。", "価格は保存済みです。実行中のプロキシには構成を適用してください。")}</p>}
        {inventory.sync.errors.length > 0 && <details>
          <summary>{copy(`Sync issues (${inventory.sync.errors.length})`, `同步问题（${inventory.sync.errors.length}）`, `同步問題（${inventory.sync.errors.length}）`, `同期の問題（${inventory.sync.errors.length}）`)}</summary>
          <ul>{inventory.sync.errors.map((message, index) => <li key={`${index}:${message}`}>{message}</li>)}</ul>
        </details>}
        <Table aria-label={copy("Configured model prices", "已接入模型价格", "已接入模型價格", "接続済みモデル価格")}>
          <TableHeader><TableRow>
            <TableHead>{copy("Provider / model", "供应商 / 模型", "供應商 / 模型", "プロバイダー / モデル")}</TableHead>
            <TableHead>{copy("Input", "输入", "輸入", "入力")}</TableHead>
            <TableHead>{copy("Output", "输出", "輸出", "出力")}</TableHead>
            <TableHead>{copy("Cache read", "缓存读", "快取讀取", "キャッシュ読み込み")}</TableHead>
            <TableHead>{copy("Cache write", "缓存写", "快取寫入", "キャッシュ書き込み")}</TableHead>
            <TableHead>{copy("Reasoning", "推理", "推理", "推論")}</TableHead>
            <TableHead>{copy("Source / updated", "来源 / 更新时间", "來源 / 更新時間", "出典 / 更新時刻")}</TableHead>
            <TableHead>{copy("Actions", "操作", "動作", "アクション")}</TableHead>
          </TableRow></TableHeader>
          <TableBody>{offerings.map((offering) => <TableRow key={offering.key}>
            <TableCell><div>{offering.upstream}</div><code>{offering.model}</code></TableCell>
            <TableCell>{offering.price ? displayRate(offering.price.input_per_mtok) : "—"}</TableCell>
            <TableCell>{offering.price ? displayRate(offering.price.output_per_mtok) : "—"}</TableCell>
            <TableCell>{offering.price ? displayRate(offering.price.cache_read_per_mtok) : "—"}</TableCell>
            <TableCell>{offering.price ? displayRate(offering.price.cache_write_per_mtok) : "—"}</TableCell>
            <TableCell>{!offering.price ? "—" : offering.price.reasoning_per_mtok == null ? copy("Same as output", "跟随输出", "跟隨輸出", "出力に従う") : displayRate(offering.price.reasoning_per_mtok)}</TableCell>
            <TableCell><div>{sourceLabel(offering.source)}</div>{offering.fetched_at_ms != null && <time dateTime={new Date(offering.fetched_at_ms).toISOString()}>{timeLabel(offering.fetched_at_ms)}</time>}</TableCell>
            <TableCell><div className="flex gap-2">
              <Button variant="outline" size="sm" aria-label={copy(`Edit ${offering.key}`, `编辑 ${offering.key}`, `編輯 ${offering.key}`, `編集 ${offering.key}`)} disabled={busy} onClick={() => dirty ? confirmNavigation(() => edit(offering.key, offering.price)) : edit(offering.key, offering.price)}>{copy("Edit", "编辑", "編輯", "編集")}</Button>
              {table?.models[offering.key] && <Button variant="outline" size="sm" aria-label={copy(`Delete ${offering.key}`, `删除 ${offering.key}`, `刪除 ${offering.key}`, `削除 ${offering.key}`)} disabled={busy || dirty} onClick={() => void remove(offering.key)}>{copy("Delete", "删除", "刪除", "削除")}</Button>}
            </div></TableCell>
          </TableRow>)}</TableBody>
        </Table>
      </div>}

      {table && Object.keys(table.models).length === 0 && (
        <div className="empty sm">{copy(
          "No model prices configured. Cost remains unknown for models without a price.",
          "尚未配置模型价格；缺失模型的成本保持未知。", "尚未設定模型價格；未設定模型的成本保持未知。", "モデルの価格が設定されていません。価格が設定されていないモデルのコストは未知のままです。"
        )}</div>
      )}
      {table && legacyPrices.length > 0 && (
        <div className="overflow-x-auto">
        <table className="grid-table price-table">
          <caption>{copy("Other saved prices (including generic references)", "其他已保存价格（含通用参考价）", "其他已儲存價格（含通用參考價）", "その他の保存済み価格（共通参考価格を含む）")}</caption>
          <thead>
            <tr>
              <th>{copy("Model", "模型", "模型", "モデル")}</th>
              <th>{copy("Input", "输入", "輸入", "入力")}</th>
              <th>{copy("Output", "输出", "輸出", "出力")}</th>
              <th>{copy("Cache read", "缓存读", "快取讀取", "キャッシュ読み込み")}</th>
              <th>{copy("Cache write", "缓存写", "快取寫入", "キャッシュ書き込み")}</th>
              <th>{copy("Reasoning", "推理", "推理", "推論")}</th>
              <th>{copy("Actions", "操作", "動作", "アクション")}</th>
            </tr>
          </thead>
          <tbody>
            {legacyPrices.map(([name, price]) => (
              <tr key={name}>
                <td className="mono">{name}</td>
                <td>{displayRate(price.input_per_mtok)}</td>
                <td>{displayRate(price.output_per_mtok)}</td>
                <td>{displayRate(price.cache_read_per_mtok)}</td>
                <td>{displayRate(price.cache_write_per_mtok)}</td>
                <td>{price.reasoning_per_mtok == null
                  ? copy("Same as output", "跟随输出", "跟隨輸出", "出力に従う")
                  : displayRate(price.reasoning_per_mtok)}</td>
                <td className="price-actions">
                  <button className="btn tiny" aria-label={copy(`Edit ${name}`, `编辑 ${name}`, `編輯 ${name}`, `編集 ${name}`)} disabled={busy} onClick={() => dirty ? confirmNavigation(() => edit(name, price)) : edit(name, price)}>
                    {copy("Edit", "编辑", "編輯", "編集")}
                  </button>
                  <button className="btn tiny danger" aria-label={copy(`Delete ${name}`, `删除 ${name}`, `刪除 ${name}`, `削除 ${name}`)} disabled={busy || dirty} onClick={() => remove(name)}>
                    {copy("Delete", "删除", "刪除", "削除")}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        </div>
      )}

      <p className="sub">{copy("Optional TTL prices fall back to the general cache write price when blank.", "TTL 价格可选；留空时使用通用缓存写入价格。", "TTL 價格可選；留空時使用通用快取寫入價格。", "TTL 価格は任意です。空欄の場合は通常のキャッシュ書き込み価格を使用します。")}</p>
      <div className="price-form">
        <label className="field-label price-model-field">
          {copy("Model ID", "模型 ID", "模型 ID", "モデル ID")}
          <input
            ref={modelInput}
            aria-label={copy("Model ID", "模型 ID", "模型 ID", "モデル ID")}
            className="input"
            disabled={busy}
            value={model}
            onChange={(event) => {
              // Changing the identity invalidates both an automatic suggestion
              // and any manually entered amount for the previous model.
              markDirty();
              setInput("0");
              setOutput("0");
              setCacheRead("0");
              setCacheWrite("0");
              setCacheWrite5m(""); setCacheWrite1h("");
              setReasoning("");
              setSuggestion(null);
              setTouchedModel(null);
              setLookupRequestedFor(null);
              setModel(event.target.value);
            }}
          />
        </label>
        {[
          [copy("Input price", "输入价格", "輸入價格", "入力価格"), input, setInput, false],
          [copy("Output price", "输出价格", "輸出價格", "出力価格"), output, setOutput, false],
          [copy("Cache read price", "缓存读取价格", "快取讀取價格", "キャッシュ読み取り価格"), cacheRead, setCacheRead, false],
          [copy("Cache write price", "缓存写入价格", "快取寫入價格", "キャッシュ書き込み価格"), cacheWrite, setCacheWrite, false],
          [copy("Cache write price (5 minutes)", "缓存写入价格（5 分钟）", "快取寫入價格（5 分鐘）", "キャッシュ書き込み価格（5 分）"), cacheWrite5m, setCacheWrite5m, true],
          [copy("Cache write price (1 hour)", "缓存写入价格（1 小时）", "快取寫入價格（1 小時）", "キャッシュ書き込み価格（1 時間）"), cacheWrite1h, setCacheWrite1h, true],
          [copy("Reasoning price", "推理价格", "推理價格", "推論価格"), reasoning, setReasoning, true],
        ].map(([label, value, setter, optional]) => (
          <label className="field-label" key={label as string}>
            {label as string}{optional ? setter === setReasoning
              ? copy(" (empty uses output price)", "（空=跟随输出）", "（空=使用輸出價格）", "（空=出力価格を使用）")
              : copy(" (empty uses cache write price)", "（空=通用缓存写入价）", "（空=通用快取寫入價）", "（空=通常のキャッシュ書き込み価格）") : ""}
            <input
              aria-label={label as string}
              className="input"
              disabled={busy}
              type="number"
              min="0"
              step="0.000001"
              value={value as string}
              onChange={(event) => {
                markDirty();
                setTouchedModel(model.trim());
                setSuggestion(null);
                (setter as (value: string) => void)(event.target.value);
              }}
            />
          </label>
        ))}
        <div className="price-save-action">
          <button
            className="btn"
            disabled={busy || !table || model.trim().length === 0 || table.models[model.trim()] != null}
            onClick={() => setLookupRequestedFor(model.trim())}
          >
            {copy("Look up public price", "查询公开价格", "查詢公開價格", "公開価格を照会")}
          </button>
          <button className="btn primary" disabled={busy || !table} onClick={save}>
            {copy("Save new version", "保存新版本", "儲存新版本", "新しいバージョンを保存")}
          </button>
          {dirty && <Button type="button" variant="outline" size="sm" disabled={working} onClick={() => void cancelEdit()}>
            {copy("Cancel editing", "取消编辑", "取消編輯", "編集をキャンセル")}
          </Button>}
        </div>
      </div>
      {suggestion && (
        <div className="banner">
          {copy(
            `Prefilled from the public USD price published by ${suggestion.source} for ${suggestion.provider_name} / ${suggestion.display_name}. Review it before saving a new version.`,
            `已按 ${suggestion.source} 的 ${suggestion.provider_name} / ${suggestion.display_name} 公开美元标价预填；尚未保存，请核对后生成新版本。`, `已按 ${suggestion.source} 的 ${suggestion.provider_name} / ${suggestion.display_name} 公開美元標價預填；尚未儲存，請核對後生成新版本。`, `${suggestion.source} の ${suggestion.provider_name} / ${suggestion.display_name} 公開米ドル価格に基づき予め入力済みです；まだ保存されていませんので、保存前に確認してください。`
          )}
        </div>
      )}
      {(error || observationError) && <div className="banner err" role="alert">{error || observationError}
        <button type="button" className="btn" disabled={working} onClick={async () => {
          if (inFlight.current) return;
          inFlight.current = true; setBusy(true);
          try { await reloadInventory(); setError(""); }
          catch (value) { showError(humanizeAppError(value), "model-price-refresh"); }
          finally { inFlight.current = false; setBusy(false); }
        }}>{copy("Refresh prices", "刷新价格表", "重新整理價格表", "価格表を更新")}</button>
      </div>}
    </div>
  );
}
