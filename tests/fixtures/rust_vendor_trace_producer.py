"""Run real SDK traces with loopback-only model and tool fixtures."""

from __future__ import annotations

import asyncio
from contextlib import contextmanager
from datetime import datetime
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import sys
import threading
from typing import TypedDict

REQUEST_TEXT = "private-vendor-fixture-question"
RESPONSE_TEXT = "private-vendor-fixture-answer"
MODEL = "private-vendor-fixture-model"
KEY = "fixture-key-never-used-off-loopback"
ROOT = "private-vendor-fixture-span"
EXIT = 23


def completion(body):
    if body["model"] == "fixture-bad-model":
        return 400, "application/json", {
            "error": {"message": RESPONSE_TEXT, "type": "invalid_request_error"},
        }
    assert body["model"] == MODEL
    if body.get("stream"):
        chunks = [
            {"index": 0, "delta": {"role": "assistant", "content": RESPONSE_TEXT},
             "finish_reason": None},
            {"index": 0, "delta": {}, "finish_reason": "stop"},
        ]
        frames = [
            {"id": "chatcmpl-fixture", "object": "chat.completion.chunk", "created": 1,
             "model": MODEL, "choices": [chunk]}
            for chunk in chunks
        ]
        return 200, "text/event-stream", (
            "".join("data: " + json.dumps(frame) + "\n\n" for frame in frames)
            + "data: [DONE]\n\n"
        )
    return 200, "application/json", {
        "id": "chatcmpl-fixture", "object": "chat.completion", "created": 1,
        "model": MODEL, "choices": [{
            "index": 0, "message": {"role": "assistant", "content": RESPONSE_TEXT},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18},
    }


class ModelEndpoint(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers["Content-Length"])
        assert 0 < length < 64 * 1024
        body = json.loads(self.rfile.read(length))
        assert body["messages"][0]["content"] == REQUEST_TEXT
        if self.path == "/v1/chat/completions":
            assert self.headers["Authorization"] == "Bearer " + KEY
            status, content_type, response = completion(body)
        else:
            assert self.path == "/v1/messages"
            assert self.headers["x-api-key"] == KEY
            assert body["model"] == MODEL
            status, content_type, response = 200, "application/json", {
                "id": "msg_fixture", "type": "message", "role": "assistant", "model": MODEL,
                "content": [{"type": "text", "text": RESPONSE_TEXT}], "stop_reason": "end_turn",
                "stop_sequence": None, "usage": {"input_tokens": 9, "output_tokens": 5},
            }
        encoded = (response if isinstance(response, str) else json.dumps(response)).encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, *_):
        pass


@contextmanager
def provider_endpoint():
    with ThreadingHTTPServer(("127.0.0.1", 0), ModelEndpoint) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}"
        finally:
            server.shutdown()
            thread.join(timeout=5)


def scoped_client(vendor):
    from observatory import Client

    return Client(transport="otlp", strict=True, session_id=f"session.{vendor}",
                  harness_id="harness.codex", turn_id="turn.fixture", worker_id="worker/fixture")


def observed_sdk_spans():
    from opentelemetry import trace
    from opentelemetry.sdk.trace import SpanProcessor

    spans = []

    class Witness(SpanProcessor):
        def on_end(self, span):
            context = span.get_span_context()
            operation = span.attributes.get("gen_ai.operation.name")
            if span.name.startswith("execute_tool "):
                operation = "execute_tool"
            spans.append({
                "trace": f"{context.trace_id:032x}",
                "span": f"{context.span_id:016x}",
                "parent": f"{span.parent.span_id:016x}" if span.parent else None,
                "start": span.start_time, "end": span.end_time,
                "status": span.status.status_code.name.lower(),
                "instrumentor": span.instrumentation_scope.name, "operation": operation,
            })

    trace.get_tracer_provider().add_span_processor(Witness())
    return spans


@contextmanager
def native_telemetry(vendor):
    from opentelemetry import trace
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.export import BatchSpanProcessor

    from observatory.integrations.otlp import MetadataSpanExporter

    client = scoped_client(vendor)
    provider = TracerProvider()
    provider.add_span_processor(BatchSpanProcessor(MetadataSpanExporter(client)))
    trace.set_tracer_provider(provider)
    try:
        yield observed_sdk_spans()
    finally:
        provider.shutdown()
    assert client.last_error is None, client.last_error


def run_openai(endpoint):
    from openai import BadRequestError, OpenAI
    from opentelemetry import trace

    spans = observed_sdk_spans()
    with trace.get_tracer("agenturm.fixture").start_as_current_span(ROOT) as parent:
        parent.set_attribute("session.id", REQUEST_TEXT)
        with OpenAI(api_key=KEY, base_url=endpoint + "/v1", max_retries=0) as sdk:
            messages = [{"role": "user", "content": REQUEST_TEXT}]
            response = sdk.chat.completions.create(model=MODEL, messages=messages)
            assert response.choices[0].message.content == RESPONSE_TEXT
            stream = sdk.chat.completions.create(model=MODEL, messages=messages, stream=True)
            assert "".join(chunk.choices[0].delta.content or "" for chunk in stream) == RESPONSE_TEXT
            try:
                sdk.chat.completions.create(model="fixture-bad-model", messages=messages)
            except BadRequestError:
                pass
            else:
                raise AssertionError("fixture error was not delivered")
    return spans


def run_anthropic(endpoint):
    from anthropic import Anthropic
    from opentelemetry import trace

    spans = observed_sdk_spans()
    with trace.get_tracer("agenturm.fixture").start_as_current_span(ROOT):
        with Anthropic(api_key=KEY, base_url=endpoint, max_retries=0) as sdk:
            response = sdk.messages.create(
                model=MODEL, max_tokens=32, messages=[{"role": "user", "content": REQUEST_TEXT}],
            )
            assert response.content[0].text == RESPONSE_TEXT
    return spans


def chat_model(endpoint):
    from langchain_openai import ChatOpenAI

    return ChatOpenAI(model=MODEL, api_key=KEY, base_url=endpoint + "/v1",
                      max_retries=0, use_responses_api=False)


def run_langchain(endpoint):
    from langchain_core.prompts import ChatPromptTemplate

    spans = observed_sdk_spans()
    prompt = ChatPromptTemplate.from_messages([("human", "{question}")])
    pipeline = prompt | chat_model(endpoint)
    result = pipeline.invoke({"question": REQUEST_TEXT})
    assert result.content == RESPONSE_TEXT
    return spans


def run_langgraph(endpoint):
    from langchain_core.runnables import RunnableConfig
    from langgraph.graph import END, START, StateGraph

    from observatory.integrations.langchain import LangChainCallbackHandler

    class State(TypedDict):
        question: str
        answer: str

    model = chat_model(endpoint)

    def ask(state: State, config: RunnableConfig):
        return {"answer": model.invoke(state["question"], config=config).content}

    graph = StateGraph(State)
    graph.add_node("ask", ask)
    graph.add_edge(START, "ask")
    graph.add_edge("ask", END)
    with scoped_client("langgraph").trace(ROOT, task=REQUEST_TEXT) as run:
        callback = LangChainCallbackHandler().attach(run)
        result = graph.compile().invoke({"question": REQUEST_TEXT, "answer": ""},
                                        config={"callbacks": [callback]})
        assert result["answer"] == RESPONSE_TEXT
    return [
        {"trace": run.trace_id, "span": span.span_id, "parent": span.parent_span_id,
         "start": span.start_ns, "end": span.end_ns, "status": span.status.lower(),
         "instrumentor": "langgraph.callbacks"}
        for span in run.spans
    ]


def run_openai_agents():
    from agents import Agent, RunConfig, Runner
    from agents.decorators import tool
    from agents.testing import ScriptedModel, assistant_message, function_call
    from agents.tracing import set_trace_processors, trace
    from agents.tracing.processors import BatchTraceProcessor

    from observatory.integrations.openai_agents import OpenAIAgentsExporter

    spans = []

    def digest(value, width):
        return hashlib.sha256(str(value).encode()).hexdigest()[:width]

    class WitnessExporter(OpenAIAgentsExporter):
        def export(self, items):
            items = tuple(items)
            for span in items:
                if not hasattr(span, "span_data"):
                    continue
                spans.append({
                    "trace": digest(span.trace_id, 32), "span": digest(span.span_id, 16),
                    "parent": digest(span.parent_id, 16) if span.parent_id else None,
                    "operation": str(span.span_data.type), "instrumentor": "agents.tracing",
                    "start": int(datetime.fromisoformat(span.started_at).timestamp() * 1_000_000_000),
                    "end": int(datetime.fromisoformat(span.ended_at).timestamp() * 1_000_000_000),
                    "status": "error" if span.error else "ok",
                })
            super().export(items)

    @tool
    def lookup_fixture(question: str) -> str:
        """Return the fixture answer."""
        assert question == REQUEST_TEXT
        return RESPONSE_TEXT

    client = scoped_client("openai-agents")
    processor = BatchTraceProcessor(WitnessExporter(client), max_batch_size=128, schedule_delay=60)
    set_trace_processors([processor])
    try:
        model = ScriptedModel([
            [function_call("lookup_fixture", {"question": REQUEST_TEXT}, call_id="call-fixture")],
            [assistant_message(RESPONSE_TEXT)],
        ], emit_traces=True)
        agent = Agent(name=ROOT, model=model, tools=[lookup_fixture])
        with trace(ROOT, group_id=REQUEST_TEXT):
            result = Runner.run_sync(agent, REQUEST_TEXT, run_config=RunConfig(tracing_disabled=False))
        assert result.final_output == RESPONSE_TEXT
        model.assert_complete()
        processor.force_flush()
        assert client.last_error is None, client.last_error
    finally:
        processor.shutdown(timeout=5)
    return spans


def run_pydantic_ai():
    with native_telemetry("pydantic-ai") as spans:
        from pydantic_ai import Agent
        from pydantic_ai.capabilities import Instrumentation
        from pydantic_ai.messages import ModelResponse, TextPart, ToolCallPart
        from pydantic_ai.models.function import FunctionModel
        from pydantic_ai.usage import RequestUsage

        calls = 0

        def fake_model(_messages, _info):
            nonlocal calls
            calls += 1
            part = (ToolCallPart("lookup_fixture", {"question": REQUEST_TEXT}, tool_call_id="call-fixture")
                    if calls == 1 else TextPart(RESPONSE_TEXT))
            return ModelResponse(parts=[part], usage=RequestUsage(input_tokens=23, output_tokens=5))

        def lookup_fixture(question: str) -> str:
            assert question == REQUEST_TEXT
            return RESPONSE_TEXT

        agent = Agent(FunctionModel(fake_model, model_name=MODEL), name=ROOT,
                      tools=[lookup_fixture], capabilities=[Instrumentation()])
        result = agent.run_sync(REQUEST_TEXT)
        assert result.output == RESPONSE_TEXT
        assert calls == 2
    return spans


def run_google_adk():
    with native_telemetry("google-adk") as spans:
        from google.adk.agents import LlmAgent
        from google.adk.models.base_llm import BaseLlm
        from google.adk.models.llm_response import LlmResponse
        from google.adk.runners import InMemoryRunner
        from google.genai import types

        class FixtureModel(BaseLlm):
            model: str = MODEL
            calls: int = 0

            @classmethod
            def supported_models(cls):
                return [MODEL]

            async def generate_content_async(self, llm_request, stream=False):
                self.calls += 1
                if self.calls == 1:
                    part = types.Part(function_call=types.FunctionCall(
                        id="call-fixture", name="lookup_fixture", args={"question": REQUEST_TEXT},
                    ))
                else:
                    part = types.Part(text=RESPONSE_TEXT)
                yield LlmResponse(content=types.Content(role="model", parts=[part]),
                                  usageMetadata=types.GenerateContentResponseUsageMetadata(
                                      promptTokenCount=17, candidatesTokenCount=9, totalTokenCount=26,
                                  ))

        def lookup_fixture(question: str) -> str:
            """Return the fixture answer."""
            assert question == REQUEST_TEXT
            return RESPONSE_TEXT

        model = FixtureModel()
        agent = LlmAgent(name="fixture_agent", model=model, tools=[lookup_fixture])
        runner = InMemoryRunner(agent=agent, app_name="fixture_app")

        async def run():
            session = await runner.session_service.create_session(
                app_name="fixture_app", user_id="fixture_user", session_id="fixture_session",
            )
            events = [event async for event in runner.run_async(
                user_id="fixture_user", session_id=session.id,
                new_message=types.Content(role="user", parts=[types.Part(text=REQUEST_TEXT)]),
            )]
            return any(part.text == RESPONSE_TEXT for event in events if event.content
                       for part in (event.content.parts or ()))

        assert asyncio.run(run())
        assert model.calls == 2
    return spans


if __name__ == "__main__":
    vendor = sys.argv[1]
    http_cases = {
        "openai": run_openai, "anthropic": run_anthropic,
        "langchain": run_langchain, "langgraph": run_langgraph,
    }
    native_cases = {
        "openai-agents": run_openai_agents, "pydantic-ai": run_pydantic_ai,
        "google-adk": run_google_adk,
    }
    if vendor in http_cases:
        with provider_endpoint() as endpoint:
            observed = http_cases[vendor](endpoint)
    else:
        observed = native_cases[vendor]()
    print(json.dumps(observed))
    raise SystemExit(EXIT)
