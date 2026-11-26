#include <cstdint>
#include <fstream>
#include <iostream>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

#ifdef PDFCRAFT_MIP_SDK
#include <filesystem>
#include <future>
#include <memory>

#if defined(__unix__) || defined(__APPLE__)
#include <sys/stat.h>
#endif

#include "mip/common_types.h"
#include "mip/mip_configuration.h"
#include "mip/mip_context.h"
#include "mip/file/file_engine.h"
#include "mip/file/file_handler.h"
#include "mip/file/file_profile.h"
#include "mip/file/labeling_options.h"
#include "mip/file/protection_settings.h"
#include "mip/protection/rights.h"
#include "mip/upe/content_label.h"
#include "mip/upe/label.h"

static std::string escape_tsv(const std::string& value) {
    std::string out;
    out.reserve(value.size());
    for (char ch : value) {
        if (ch == '\\') {
            out += "\\\\";
        } else if (ch == '\t') {
            out += "\\t";
        } else if (ch == '\n') {
            out += "\\n";
        } else {
            out.push_back(ch);
        }
    }
    return out;
}

static std::string unescape_tsv(const std::string& value) {
    std::string out;
    out.reserve(value.size());
    for (std::size_t i = 0; i < value.size(); ++i) {
        if (value[i] == '\\' && i + 1 < value.size()) {
            char next = value[++i];
            if (next == 't') {
                out.push_back('\t');
            } else if (next == 'n') {
                out.push_back('\n');
            } else if (next == '\\') {
                out.push_back('\\');
            } else {
                out.push_back(next);
            }
        } else {
            out.push_back(value[i]);
        }
    }
    return out;
}

static std::vector<std::string> split_tsv(const std::string& line) {
    std::vector<std::string> fields;
    std::string field;
    for (std::size_t i = 0; i < line.size(); ++i) {
        char ch = line[i];
        if (ch == '\t') {
            fields.push_back(field);
            field.clear();
        } else {
            field.push_back(ch);
        }
    }
    fields.push_back(field);
    return fields;
}

class FixedTokenAuth final : public mip::AuthDelegate {
public:
    FixedTokenAuth(std::string rms_token, std::string sync_token)
        : rms_token_(std::move(rms_token)), sync_token_(std::move(sync_token)) {}

    bool AcquireOAuth2Token(
        const mip::Identity&,
        const OAuth2Challenge&,
        OAuth2Token& token) override {
        return AcquireOAuth2TokenImpl("", token);
    }

    bool AcquireOAuth2Token(
        const mip::Identity& identity,
        const OAuth2Challenge& challenge,
        const std::shared_ptr<void>& context,
        OAuth2Token& token) override {
        (void)identity;
        (void)context;
        const std::string resource = challenge.GetResource();
        const std::string scope = challenge.GetScope();
        const bool sync_challenge =
            resource.find("psor") != std::string::npos ||
            scope.find("UnifiedPolicy") != std::string::npos ||
            scope.find("psor") != std::string::npos;
        const std::string& selected = sync_challenge ? sync_token_ : rms_token_;
        token.SetAccessToken(selected);
        return !selected.empty();
    }

private:
    bool AcquireOAuth2TokenImpl(const std::string& unused, OAuth2Token& token) {
        (void)unused;
        token.SetAccessToken(rms_token_);
        return !rms_token_.empty();
    }

    std::string rms_token_;
    std::string sync_token_;
};

class AlwaysConsent final : public mip::ConsentDelegate {
public:
    mip::Consent GetUserConsent(const std::string&) override {
        return mip::Consent::AcceptAlways;
    }
};

class ProfileObserver final : public mip::FileProfile::Observer {
public:
    void OnLoadSuccess(const std::shared_ptr<mip::FileProfile>& profile, const std::shared_ptr<void>& context) override {
        std::static_pointer_cast<std::promise<std::shared_ptr<mip::FileProfile>>>(context)->set_value(profile);
    }
    void OnLoadFailure(const std::exception_ptr& error, const std::shared_ptr<void>& context) override {
        std::static_pointer_cast<std::promise<std::shared_ptr<mip::FileProfile>>>(context)->set_exception(error);
    }
    void OnAddEngineSuccess(const std::shared_ptr<mip::FileEngine>& engine, const std::shared_ptr<void>& context) override {
        std::static_pointer_cast<std::promise<std::shared_ptr<mip::FileEngine>>>(context)->set_value(engine);
    }
    void OnAddEngineFailure(const std::exception_ptr& error, const std::shared_ptr<void>& context) override {
        std::static_pointer_cast<std::promise<std::shared_ptr<mip::FileEngine>>>(context)->set_exception(error);
    }
};

class HandlerObserver final : public mip::FileHandler::Observer {
public:
    void OnCreateFileHandlerSuccess(
        const std::shared_ptr<mip::FileHandler>& handler,
        const std::shared_ptr<void>& context) override {
        std::static_pointer_cast<std::promise<std::shared_ptr<mip::FileHandler>>>(context)->set_value(handler);
    }
    void OnCreateFileHandlerFailure(const std::exception_ptr& error, const std::shared_ptr<void>& context) override {
        std::static_pointer_cast<std::promise<std::shared_ptr<mip::FileHandler>>>(context)->set_exception(error);
    }
    void OnCommitSuccess(bool committed, const std::shared_ptr<void>& context) override {
        std::static_pointer_cast<std::promise<bool>>(context)->set_value(committed);
    }
    void OnCommitFailure(const std::exception_ptr& error, const std::shared_ptr<void>& context) override {
        std::static_pointer_cast<std::promise<bool>>(context)->set_exception(error);
    }
    void OnGetDecryptedTemporaryFileSuccess(
        const std::string& decryptedFilePath,
        const std::shared_ptr<void>& context) override {
        std::static_pointer_cast<std::promise<std::string>>(context)->set_value(decryptedFilePath);
    }
    void OnGetDecryptedTemporaryFileFailure(
        const std::exception_ptr& error,
        const std::shared_ptr<void>& context) override {
        std::static_pointer_cast<std::promise<std::string>>(context)->set_exception(error);
    }
};

struct NativeMip {
    std::shared_ptr<mip::MipContext> context;
    std::shared_ptr<mip::FileProfile> profile;
    std::shared_ptr<mip::FileEngine> engine;
};

static NativeMip create_native_mip(
    const std::string& app_id,
    const std::string& username,
    const std::string& rms_token,
    const std::string& sync_token,
    const std::string& cache_dir) {
    mip::ApplicationInfo app_info{app_id, "PdfCraft", "0.21.1"};
    std::filesystem::create_directories(cache_dir);
    auto configuration = std::make_shared<mip::MipConfiguration>(
        app_info,
        cache_dir,
        mip::LogLevel::Error,
        false,
        mip::CacheStorageType::OnDiskEncrypted);
    auto context = mip::MipContext::Create(configuration);
    mip::FileProfile::Settings profile_settings(
        context,
        mip::CacheStorageType::OnDiskEncrypted,
        std::make_shared<AlwaysConsent>(),
        std::make_shared<ProfileObserver>());
    auto profile_promise = std::make_shared<std::promise<std::shared_ptr<mip::FileProfile>>>();
    auto profile_future = profile_promise->get_future();
    mip::FileProfile::LoadAsync(profile_settings, profile_promise);
    auto profile = profile_future.get();

    auto auth = std::make_shared<FixedTokenAuth>(rms_token, sync_token);
    mip::FileEngine::Settings engine_settings(
        mip::Identity(username),
        auth,
        "",
        "en-US",
        false);
    engine_settings.SetEngineId(username.empty() ? "pdfcraft" : username);
    auto engine_promise = std::make_shared<std::promise<std::shared_ptr<mip::FileEngine>>>();
    auto engine_future = engine_promise->get_future();
    profile->AddEngineAsync(engine_settings, engine_promise);
    auto engine = engine_future.get();
    return {context, profile, engine};
}

static std::shared_ptr<mip::FileHandler> create_handler(
    const std::shared_ptr<mip::FileEngine>& engine,
    const std::string& input_path,
    const std::string& audit_path) {
    auto promise = std::make_shared<std::promise<std::shared_ptr<mip::FileHandler>>>();
    auto future = promise->get_future();
    engine->CreateFileHandlerAsync(
        input_path,
        audit_path,
        true,
        std::make_shared<HandlerObserver>(),
        promise);
    return future.get();
}

static bool commit_handler(
    const std::shared_ptr<mip::FileHandler>& handler,
    const std::string& output_path) {
    if (!handler->IsModified()) {
        return false;
    }
    auto promise = std::make_shared<std::promise<bool>>();
    auto future = promise->get_future();
    handler->CommitAsync(output_path, promise);
    return future.get();
}

static std::string rights_line(const std::shared_ptr<mip::ProtectionHandler>& protection) {
    if (!protection) {
        return "0\t1\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0";
    }
    auto check = [&](const std::string& right) -> std::string {
        return protection->AccessCheck(right) ? "1" : "0";
    };
    const bool save_right = protection->AccessCheck(mip::rights::Edit());
    return check(mip::rights::Owner()) + "\t" +
        check(mip::rights::View()) + "\t" +
        check(mip::rights::Edit()) + "\t" +
        (save_right ? "1" : "0") + "\t" +
        check(mip::rights::Comment()) + "\t" +
        check(mip::rights::Extract()) + "\t" +
        check(mip::rights::Export()) + "\t" +
        check(mip::rights::Print()) + "\t" +
        check(mip::rights::Forward()) + "\t" +
        check(mip::rights::Reply()) + "\t" +
        check(mip::rights::ReplyAll()) + "\t" +
        check(mip::rights::ViewRightsData()) + "\t" +
        check(mip::rights::EditRightsData()) + "\t" +
        check(mip::rights::ObjectModel()) + "\t" +
        check(mip::rights::DocumentEdit());
}

static std::string content_label_fields(
    const std::shared_ptr<mip::FileHandler>& handler) {
    auto label = handler->GetLabel();
    if (!label) {
        return "\t\t\t\t";
    }
    auto current = label->GetLabel();
    if (!current) {
        return "\t\t\t\t";
    }
    return escape_tsv(current->GetId()) + "\t" +
        escape_tsv(current->GetName()) + "\t" +
        escape_tsv(current->GetDescription()) + "\t" +
        escape_tsv(current->GetTooltip()) + "\t" +
        escape_tsv(current->GetColor());
}

static void copy_file(const std::string& input, const std::string& output) {
    if (std::filesystem::file_size(input) > 512ULL * 1024ULL * 1024ULL) {
        throw std::runtime_error("Microsoft Purview staging exceeded the 512 MiB limit");
    }
    std::ifstream source(input, std::ios::binary);
    std::ofstream destination(output, std::ios::binary | std::ios::trunc);
    if (!source || !destination) {
        throw std::runtime_error("Microsoft Purview could not open the staged file");
    }
    destination << source.rdbuf();
    if (source.bad() || !destination) {
        throw std::runtime_error("Microsoft Purview could not copy the staged file");
    }
    destination.flush();
    if (!destination) {
        throw std::runtime_error("Microsoft Purview could not flush the staged file");
    }
    destination.close();
    if (!destination) {
        throw std::runtime_error("Microsoft Purview could not close the staged file");
    }
#if defined(__unix__) || defined(__APPLE__)
    if (::chmod(output.c_str(), 0600) != 0) {
        throw std::runtime_error("Microsoft Purview could not restrict the staged file");
    }
#endif
}

static void handle_open(const std::vector<std::string>& fields) {
    if (fields.size() < 9) {
        std::cout << "DENIED\tinvalid open request\n";
        return;
    }
    std::string app_id = unescape_tsv(fields[1]);
    std::string username = unescape_tsv(fields[2]);
    std::string rms_token = unescape_tsv(fields[3]);
    std::string sync_token = unescape_tsv(fields[4]);
    std::string input = unescape_tsv(fields[5]);
    std::string output = unescape_tsv(fields[6]);
    std::string audit = unescape_tsv(fields[7]);
    std::string cache = unescape_tsv(fields[8]);

    try {
        mip::ApplicationInfo app_info{app_id, "PdfCraft", "0.21.1"};
        auto mip_configuration = std::make_shared<mip::MipConfiguration>(
            app_info,
            cache,
            mip::LogLevel::Error,
            false,
            mip::CacheStorageType::OnDiskEncrypted);
        auto mip_context = mip::MipContext::Create(mip_configuration);

        bool labeled_or_protected = mip::FileHandler::IsLabeledOrProtected(input, mip_context);
        if (!labeled_or_protected) {
            std::cout << "NONE\n";
            return;
        }
        if (rms_token.empty() || sync_token.empty()) {
            std::cout << "SIGNIN\n";
            return;
        }

        auto native = create_native_mip(app_id, username, rms_token, sync_token, cache);
        auto handler = create_handler(native.engine, input, audit);
        auto protection = handler->GetProtection();
        bool protected_file = protection != nullptr;
        if (protected_file) {
            if (!protection->AccessCheck(mip::rights::View())) {
                std::cout << "NO_ACCESS\n";
                native.context->ShutDown();
                return;
            }
            auto promise = std::make_shared<std::promise<std::string>>();
            auto future = promise->get_future();
            handler->GetDecryptedTemporaryFileAsync(promise);
            std::string decrypted = future.get();
            copy_file(decrypted, output);
            std::error_code removed_error;
            std::filesystem::remove(decrypted, removed_error);
            if (removed_error) {
                throw std::runtime_error("Microsoft Purview could not remove its decrypted temporary file");
            }
        } else {
            copy_file(input, output);
        }
        auto label_fields = content_label_fields(handler);
        auto rights_fields = rights_line(protection);
        std::string owner;
        std::string content_id;
        if (protection) {
            owner = protection->GetOwner();
            content_id = protection->GetContentId();
        }
        bool labeling_required = native.engine->IsLabelingRequired();
        std::string default_label_id;
        auto default_label = native.engine->GetDefaultSensitivityLabel();
        if (default_label) {
            default_label_id = default_label->GetId();
        }
        std::cout << "OK\t"
                  << (protected_file ? "PROTECTED" : "NONE") << "\t"
                  << label_fields << "\t"
                  << rights_fields << "\t"
                  << escape_tsv(owner) << "\t"
                  << escape_tsv(content_id) << "\t"
                  << (labeling_required ? "1" : "0") << "\t"
                  << escape_tsv(default_label_id) << "\t"
                  << "0\n";
        native.context->ShutDown();
    } catch (const std::exception& error) {
        std::cout << "DENIED\t" << escape_tsv(error.what()) << "\n";
    } catch (...) {
        std::cout << "DENIED\tMicrosoft Purview could not open the document.\n";
    }
}

static void handle_labels(const std::vector<std::string>& fields) {
    if (fields.size() < 6) {
        std::cout << "DENIED\tinvalid labels request\n";
        return;
    }
    try {
        auto native = create_native_mip(
            unescape_tsv(fields[1]),
            unescape_tsv(fields[2]),
            unescape_tsv(fields[3]),
            unescape_tsv(fields[4]),
            unescape_tsv(fields[5]));
        auto labels = native.engine->ListSensitivityLabels();
        for (const auto& label : labels) {
            std::cout << "LABEL\t"
                      << escape_tsv(label->GetId()) << "\t"
                      << escape_tsv(label->GetName()) << "\t"
                      << escape_tsv(label->GetDescription()) << "\t"
                      << escape_tsv(label->GetTooltip()) << "\t"
                      << escape_tsv(label->GetColor()) << "\t"
                      << "-\t"
                      << label->GetSensitivity() << "\t"
                      << (label->IsActive() ? "1" : "0") << "\t"
                      << (label->HasRightsManagementPolicy() ? "1" : "0") << "\n";
            for (const auto& child : label->GetChildren()) {
                std::cout << "LABEL\t"
                          << escape_tsv(child->GetId()) << "\t"
                          << escape_tsv(child->GetName()) << "\t"
                          << escape_tsv(child->GetDescription()) << "\t"
                          << escape_tsv(child->GetTooltip()) << "\t"
                          << escape_tsv(child->GetColor()) << "\t"
                          << escape_tsv(label->GetId()) << "\t"
                          << child->GetSensitivity() << "\t"
                          << (child->IsActive() ? "1" : "0") << "\t"
                          << (child->HasRightsManagementPolicy() ? "1" : "0") << "\n";
            }
        }
        std::cout << "END\n";
        native.context->ShutDown();
    } catch (const std::exception& error) {
        std::cout << "DENIED\t" << escape_tsv(error.what()) << "\n";
    }
}

static std::string open_and_print_result(
    const std::string& app_id,
    const std::string& username,
    const std::string& token,
    const std::string& input,
    const std::string& output) {
    std::string request = "open\t" + app_id + "\t" + unescape_tsv(username) + "\t" +
        unescape_tsv(token) + "\t" + unescape_tsv(input) + "\t" + unescape_tsv(output) + "\t" + unescape_tsv(output + ".audit");
    (void)request;
    return "";
}

static void handle_apply_label(const std::vector<std::string>& fields) {
    if (fields.size() < 10) {
        std::cout << "DENIED\tinvalid apply-label request\n";
        return;
    }
    try {
        auto native = create_native_mip(
            unescape_tsv(fields[1]),
            unescape_tsv(fields[2]),
            unescape_tsv(fields[3]),
            unescape_tsv(fields[4]),
            unescape_tsv(fields[9]));
        std::string input = unescape_tsv(fields[5]);
        std::string output = unescape_tsv(fields[6]);
        std::string label_id = unescape_tsv(fields[7]);
        std::string justification = unescape_tsv(fields[8]);
        auto handler = create_handler(native.engine, input, input + ".audit");
        mip::LabelingOptions options(mip::AssignmentMethod::PRIVILEGED);
        if (!justification.empty()) {
            options.SetDowngradeJustification(true, justification);
        }
        handler->SetLabel(native.engine->GetLabelById(label_id), options, mip::ProtectionSettings());
        if (!commit_handler(handler, output)) {
            std::cout << "DENIED\tThe Microsoft Purview SDK did not commit the label.\n";
            return;
        }
        std::string working = output + ".working";
        auto output_handler = create_handler(native.engine, output, output + ".audit");
        auto protection = output_handler->GetProtection();
        if (protection) {
            auto promise = std::make_shared<std::promise<std::string>>();
            auto future = promise->get_future();
            output_handler->GetDecryptedTemporaryFileAsync(promise);
            std::string decrypted = future.get();
            copy_file(decrypted, working);
            std::error_code removed_error;
            std::filesystem::remove(decrypted, removed_error);
            if (removed_error) {
                throw std::runtime_error("Microsoft Purview could not remove its decrypted temporary file");
            }
        } else {
            copy_file(output, working);
        }
        auto label_fields = content_label_fields(output_handler);
        std::string rights = rights_line(protection);
        std::string owner;
        std::string content_id;
        if (protection) {
            owner = protection->GetOwner();
            content_id = protection->GetContentId();
        }
        std::cout << "OK\t"
                  << (protection ? "PROTECTED" : "NONE") << "\t"
                  << label_fields << "\t"
                  << rights << "\t"
                  << escape_tsv(owner) << "\t"
                  << escape_tsv(content_id) << "\t"
                  << (native.engine->IsLabelingRequired() ? "1" : "0") << "\t"
                  << escape_tsv(native.engine->GetDefaultSensitivityLabel() ? native.engine->GetDefaultSensitivityLabel()->GetId() : "") << "\t"
                  << "0\t"
                  << escape_tsv(working) << "\t"
                  << escape_tsv(output) << "\n";
        native.context->ShutDown();
    } catch (const std::exception& error) {
        std::string message = error.what();
        if (message.find("Justification") != std::string::npos ||
            message.find("justification") != std::string::npos) {
            std::cout << "JUSTIFICATION\n";
        } else {
            std::cout << "DENIED\t" << escape_tsv(message) << "\n";
        }
    }
}

static void handle_remove_label(const std::vector<std::string>& fields) {
    if (fields.size() < 9) {
        std::cout << "DENIED\tinvalid remove-label request\n";
        return;
    }
    try {
        auto native = create_native_mip(
            unescape_tsv(fields[1]),
            unescape_tsv(fields[2]),
            unescape_tsv(fields[3]),
            unescape_tsv(fields[4]),
            unescape_tsv(fields[8]));
        std::string input = unescape_tsv(fields[5]);
        std::string output = unescape_tsv(fields[6]);
        std::string justification = unescape_tsv(fields[7]);
        auto handler = create_handler(native.engine, input, input + ".audit");
        mip::LabelingOptions options(mip::AssignmentMethod::PRIVILEGED);
        if (!justification.empty()) {
            options.SetDowngradeJustification(true, justification);
        }
        handler->DeleteLabel(options);
        if (!commit_handler(handler, output)) {
            std::cout << "DENIED\tThe Microsoft Purview SDK did not remove the label.\n";
            return;
        }
        auto output_handler = create_handler(native.engine, output, output + ".audit");
        auto protection = output_handler->GetProtection();
        auto label_fields = content_label_fields(output_handler);
        auto rights_fields = rights_line(protection);
        std::string owner;
        std::string content_id;
        if (protection) {
            owner = protection->GetOwner();
            content_id = protection->GetContentId();
        }
        std::cout << "OK\t"
                  << (protection ? "PROTECTED" : "NONE") << "\t"
                  << label_fields << "\t"
                  << rights_fields << "\t"
                  << escape_tsv(owner) << "\t"
                  << escape_tsv(content_id) << "\t"
                  << (native.engine->IsLabelingRequired() ? "1" : "0") << "\t"
                  << escape_tsv(native.engine->GetDefaultSensitivityLabel() ? native.engine->GetDefaultSensitivityLabel()->GetId() : "") << "\t"
                  << "0\t"
                  << escape_tsv(output) << "\n";
        native.context->ShutDown();
    } catch (const std::exception& error) {
        std::cout << "DENIED\t" << escape_tsv(error.what()) << "\n";
    }
}

static void handle_remove_protection(const std::vector<std::string>& fields) {
    if (fields.size() < 8) {
        std::cout << "DENIED\tinvalid remove-protection request\n";
        return;
    }
    try {
        auto native = create_native_mip(
            unescape_tsv(fields[1]),
            unescape_tsv(fields[2]),
            unescape_tsv(fields[3]),
            unescape_tsv(fields[4]),
            unescape_tsv(fields[7]));
        std::string input = unescape_tsv(fields[5]);
        std::string output = unescape_tsv(fields[6]);
        auto handler = create_handler(native.engine, input, input + ".audit");
        auto protection = handler->GetProtection();
        if (!protection || (!protection->AccessCheck(mip::rights::Export()) &&
                            !protection->AccessCheck(mip::rights::Owner()))) {
            std::cout << "EXPORT_REQUIRED\n";
            return;
        }
        handler->RemoveProtection();
        if (!commit_handler(handler, output)) {
            std::cout << "DENIED\tThe Microsoft Purview SDK did not remove protection.\n";
            return;
        }
        auto output_handler = create_handler(native.engine, output, output + ".audit");
        auto protection_after = output_handler->GetProtection();
        auto label_fields = content_label_fields(output_handler);
        auto rights_fields = rights_line(protection_after);
        std::string owner;
        std::string content_id;
        if (protection_after) {
            owner = protection_after->GetOwner();
            content_id = protection_after->GetContentId();
        }
        std::cout << "OK\tNONE\t"
                  << label_fields << "\t"
                  << rights_fields << "\t"
                  << escape_tsv(owner) << "\t"
                  << escape_tsv(content_id) << "\t"
                  << (native.engine->IsLabelingRequired() ? "1" : "0") << "\t"
                  << escape_tsv(native.engine->GetDefaultSensitivityLabel() ? native.engine->GetDefaultSensitivityLabel()->GetId() : "") << "\t"
                  << "0\t"
                  << escape_tsv(output) << "\n";
        native.context->ShutDown();
    } catch (const std::exception& error) {
        std::cout << "DENIED\t" << escape_tsv(error.what()) << "\n";
    }
}

static void handle_republish(const std::vector<std::string>& fields) {
    if (fields.size() < 9) {
        std::cout << "DENIED\tinvalid republish request\n";
        return;
    }
    try {
        auto native = create_native_mip(
            unescape_tsv(fields[1]),
            unescape_tsv(fields[2]),
            unescape_tsv(fields[3]),
            unescape_tsv(fields[4]),
            unescape_tsv(fields[8]));
        std::string original = unescape_tsv(fields[5]);
        std::string working = unescape_tsv(fields[6]);
        std::string output = unescape_tsv(fields[7]);
        auto original_handler = create_handler(native.engine, original, original + ".audit");
        auto original_protection = original_handler->GetProtection();
        if (!original_protection) {
            copy_file(working, output);
            std::cout << "OK\n";
            return;
        }
        auto working_handler = create_handler(native.engine, working, working + ".audit");
        working_handler->SetProtection(original_protection);
        if (!commit_handler(working_handler, output)) {
            std::cout << "DENIED\tThe Microsoft Purview SDK did not republish the document.\n";
            return;
        }
        std::cout << "OK\n";
        native.context->ShutDown();
    } catch (const std::exception& error) {
        std::cout << "DENIED\t" << escape_tsv(error.what()) << "\n";
    }
}

int main() {
    std::string line;
    if (!std::getline(std::cin, line)) {
        return 1;
    }
    auto fields = split_tsv(line);
    if (fields.empty()) {
        return 1;
    }
    if (fields[0] == "open") {
        handle_open(fields);
    } else if (fields[0] == "labels") {
        handle_labels(fields);
    } else if (fields[0] == "apply-label") {
        handle_apply_label(fields);
    } else if (fields[0] == "remove-label") {
        handle_remove_label(fields);
    } else if (fields[0] == "remove-protection") {
        handle_remove_protection(fields);
    } else if (fields[0] == "republish") {
        handle_republish(fields);
    } else {
        std::cout << "DENIED\tunknown Microsoft Purview helper request\n";
    }
    return 0;
}

#else

int main() {
    std::string line;
    if (!std::getline(std::cin, line)) {
        return 1;
    }
    std::string command;
    for (char ch : line) {
        if (ch == '\t') {
            break;
        }
        command.push_back(ch);
    }
    if (command == "open") {
        std::cout << "NONE\n";
    } else if (command == "labels") {
        std::cout << "END\n";
    } else {
        std::cout << "DENIED\tMicrosoft Purview MIP SDK support is not configured in this build.\n";
    }
    return 0;
}

#endif
