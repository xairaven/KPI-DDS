# HiveMind (swarm emulator) from the lecturer's repo.
# The lecturer publishes no image for it, so we build it from the submodule source.
# Build context: infra/hiveemulator/upstream/src/CommunicationControl

# Stage 1: compile with the full .NET SDK (~800 MB, thrown away afterwards).
FROM mcr.microsoft.com/dotnet/sdk:8.0 AS build
WORKDIR /src
COPY . .
RUN dotnet publish -c Release -o /published \
    DevOpsProject.HiveMind.API/DevOpsProject.HiveMind.API.csproj

# Stage 2: run with the small ASP.NET runtime only, copying in the compiled output.
FROM mcr.microsoft.com/dotnet/aspnet:8.0
WORKDIR /app
COPY --from=build /published ./
# Built-in non-root user of the official .NET images.
USER $APP_UID
EXPOSE 5149
ENTRYPOINT ["dotnet", "DevOpsProject.HiveMind.API.dll"]
