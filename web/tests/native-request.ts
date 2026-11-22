type Message = {
  role: string;
  content: string | null | { type: string; text?: string }[];
};

export function nativeUserData(messages: Message[]) {
  return messages
    .filter((message) => message.role === "user")
    .flatMap(({ content }) =>
      typeof content === "string"
        ? [content]
        : (content ?? []).flatMap((part) =>
            part.type === "text" && part.text ? [part.text] : [],
          ),
    )
    .map((text) => JSON.parse(text));
}
