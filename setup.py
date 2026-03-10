from setuptools import setup, find_packages

setup(
    name="agent-memory-system",
    version="0.1.0",
    packages=find_packages(),
    install_requires=[
        "networkx>=3.0",
        "chromadb>=0.4.0",
        "pydantic>=2.0",
        "langgraph",
        "langchain",
        "litellm",
        "numpy",
        "python-dotenv"
    ],
)
