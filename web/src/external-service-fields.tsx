import { useState } from "react";
import { Field, useI18n } from "./ui";
import { Check, Group, Hint, pairClass } from "./components/patterns";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";

/** The Registry form writes one destination and explicit forwarding authority. */
export function ExternalServiceFields() {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [base, setBase] = useState("");
  const [methods, setMethods] = useState(["GET"]);
  const [secret, setSecret] = useState("");
  const [authentication, setAuthentication] = useState("bearer");
  const [header, setHeader] = useState("X-API-Key");
  const [lower, setLower] = useState(false);
  const [sent, setSent] = useState(1 << 30);
  const [received, setReceived] = useState(1 << 30);
  const [requests, setRequests] = useState(1000);
  const config = {
    base_url: base,
    methods,
    ...(secret ? { secret } : {}),
    authentication:
      authentication === "bearer"
        ? { type: "bearer" }
        : { type: "header", name: header },
    ...(lower
      ? { limits: { sent_bytes: sent, received_bytes: received, requests } }
      : {}),
  };
  const limits = [
    { label: ja ? "送信バイト数" : "Bytes sent", value: sent, change: setSent },
    {
      label: ja ? "受信バイト数" : "Bytes received",
      value: received,
      change: setReceived,
    },
    {
      label: ja ? "リクエスト数" : "Requests",
      value: requests,
      change: setRequests,
    },
  ];
  return (
    <>
      <input type="hidden" name="config" value={JSON.stringify(config)} />
      <Field label={ja ? "接続先の HTTPS URL" : "Destination HTTPS URL"}>
        <Input
          type="url"
          required
          value={base}
          placeholder="https://api.example.com/"
          pattern="https://.+"
          onChange={(event) => setBase(event.target.value)}
        />
      </Field>
      <Group legend={ja ? "許可するメソッド" : "Allowed methods"}>
        {["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"].map(
          (method) => (
            <Check
              key={method}
              checked={methods.includes(method)}
              onChange={(event) =>
                setMethods(
                  event.target.checked
                    ? [...methods, method]
                    : methods.filter((item) => item !== method),
                )
              }
            >
              {method}
            </Check>
          ),
        )}
      </Group>
      <Field label={ja ? "Tenant Secret（任意）" : "Tenant Secret (optional)"}>
        <Input
          value={secret}
          pattern="[a-z0-9][a-z0-9_-]{0,63}"
          onChange={(event) => setSecret(event.target.value)}
        />
      </Field>
      {secret && (
        <>
          <Field label={ja ? "認証方法" : "Authentication"}>
            <NativeSelect
              value={authentication}
              onChange={(event) => setAuthentication(event.target.value)}
            >
              <option value="bearer">Bearer token</option>
              <option value="header">
                {ja ? "指定したヘッダー" : "Named header"}
              </option>
            </NativeSelect>
          </Field>
          {authentication === "header" && (
            <Field label={ja ? "ヘッダー名" : "Header name"}>
              <Input
                required
                value={header}
                onChange={(event) => setHeader(event.target.value)}
              />
            </Field>
          )}
        </>
      )}
      <Check
        checked={lower}
        onChange={(event) => setLower(event.target.checked)}
      >
        {ja
          ? "通信量とリクエスト数の上限を下げる"
          : "Set lower transfer and request limits"}
      </Check>
      {lower && (
        <div className={pairClass}>
          {limits.map(({ label, value, change }) => (
            <Field key={label} label={label}>
              <Input
                type="number"
                min={1}
                step={1}
                required
                value={value}
                onChange={(event) => change(event.target.valueAsNumber)}
              />
            </Field>
          ))}
        </div>
      )}
      <Hint>
        {ja
          ? "Sandboxed code は Working Area 内の Tenant データを含め、読み取れる情報を送信できます。リクエストごとの承認はありません。"
          : "Sandboxed code may send anything it can read, including Tenant data in the Working Area. Individual requests are not approved."}
      </Hint>
    </>
  );
}
