import { useState } from "react";
import { useInfiniteQuery } from "@tanstack/react-query";
import { useI18n } from "../ui";
import { post } from "./client";
type Skill = {
  skill_id: string;
  name: string;
  description: string;
  digest: string;
  origin: string;
};
type Read = {
  content?: string;
  next_offset?: number;
  encoding?: string;
  path: string;
  files?: { path: string; size: number }[];
};
export function SkillFiles({ run }: { run: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [enabled, setEnabled] = useState(false);
  const [selected, setSelected] = useState<Skill>();
  const [inventory, setInventory] = useState<NonNullable<Read["files"]>>([]);
  const [read, setRead] = useState<Read>();
  const [error, setError] = useState("");
  const query = useInfiniteQuery({
    queryKey: ["core-skills", run],
    initialPageParam: 0,
    enabled,
    retry: false,
    queryFn: ({ pageParam }) =>
      post<{ skills: Skill[]; next_cursor: number | null }>(
        `/runs/${run}/skills/list`,
        { cursor: pageParam },
      ),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
  });
  const load = async (skill: Skill, path?: string, offset = 0) => {
    setError("");
    try {
      const result = await post<Read>(
        `/runs/${run}/skills/${path ? "read" : "load"}`,
        path
          ? { skill_id: skill.skill_id, digest: skill.digest, path, offset }
          : { skill_id: skill.skill_id, expected_digest: skill.digest },
      );
      setSelected(skill);
      setRead(result);
      if (result.files) setInventory(result.files);
    } catch (e) {
      setError(String(e));
    }
  };
  return (
    <details className="core-panel">
      <summary>Skills</summary>
      <button
        type="button"
        onClick={() => {
          setEnabled(true);
          if (enabled) void query.refetch();
        }}
      >
        {ja ? "利用可能な Skill を表示" : "List available Skills"}
      </button>
      {query.data?.pages
        .flatMap((page) => page.skills)
        .map((skill) => (
          <article key={skill.skill_id}>
            <strong>{skill.name}</strong>
            <p>{skill.description}</p>
            <small>
              {skill.origin} · {skill.digest}
            </small>
            <button type="button" onClick={() => void load(skill)}>
              {ja ? "内容を読み込む" : "Load instructions"}
            </button>
          </article>
        ))}
      {query.hasNextPage && (
        <button type="button" onClick={() => void query.fetchNextPage()}>
          {ja ? "さらに表示" : "Load more"}
        </button>
      )}
      {selected && (
        <div>
          <p>
            {ja
              ? "ファイルの読込みはスクリプトを実行しません。"
              : "Reading a file does not execute its scripts."}
          </p>
          <ul>
            {inventory.map((file) => (
              <li key={file.path}>
                <button
                  type="button"
                  onClick={() => void load(selected, file.path)}
                >
                  {file.path}
                </button>{" "}
                · {file.size} bytes
              </li>
            ))}
          </ul>
          {read?.content && <pre>{read.content}</pre>}
          {read?.encoding === "binary" && (
            <p>
              {ja
                ? "バイナリファイルのため、テキスト表示はできません。"
                : "This binary file has no text representation."}
            </p>
          )}
          {read?.next_offset != null && (
            <button
              type="button"
              onClick={() => void load(selected, read.path, read.next_offset)}
            >
              {ja ? "内容の続き" : "Continue reading"}
            </button>
          )}
        </div>
      )}
      {(error || query.error) && (
        <p role="alert">{error || query.error?.message}</p>
      )}
    </details>
  );
}
